//! Flash Loan Executor - Phase 4
//!
//! This module handles the Flash Loan integration for executing arbitrage.
//! Currently supports:
//! - Balancer V2 (0% fee - recommended!)
//! - Aave V3 (0.05% fee)
//!
//! The executor contract must be deployed on-chain before production use.

use alloy_primitives::{Address, Bytes, U256, address};
use alloy_sol_types::{sol, SolCall};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types::TransactionRequest;
use eyre::{eyre, Result};
use tracing::{debug, info, warn};

use crate::brain::ArbitrageCycle;
use crate::config::{Config, FlashLoanProvider};
use crate::cartographer::Dex;

// ============================================
// CONTRACT ADDRESSES (Ethereum Mainnet)
// ============================================

/// Balancer V2 Vault (0% flash loan fee!)
const BALANCER_VAULT: Address = address!("BA12222222228d8Ba445958a75a0704d566BF2C8");

/// Aave V3 Pool (0.05% fee)
const AAVE_V3_POOL: Address = address!("87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2");

/// Uniswap V3 Factory (for flash swaps)
const UNISWAP_V3_FACTORY: Address = address!("1F98431c8aD98523631AE4a59f267346ea31F984");

// ============================================
// SOLIDITY INTERFACES
// ============================================

sol! {
    /// Balancer V2 Vault interface for flash loans
    interface IBalancerVault {
        function flashLoan(
            address recipient,
            address[] memory tokens,
            uint256[] memory amounts,
            bytes memory userData
        ) external;
    }
    
    /// Aave V3 Pool interface for flash loans
    interface IAavePool {
        function flashLoan(
            address receiverAddress,
            address[] calldata assets,
            uint256[] calldata amounts,
            uint256[] calldata interestRateModes,
            address onBehalfOf,
            bytes calldata params,
            uint16 referralCode
        ) external;
    }
    
    /// Our executor contract interface
    interface IArbitrageExecutor {
        /// Execute an arbitrage cycle with a flash loan
        function executeArbitrage(
            address[] calldata path,
            address[] calldata pools,
            uint8[] calldata dexTypes,
            uint256 inputAmount,
            uint256 minOutput
        ) external returns (uint256 profit);
        
        /// Withdraw accumulated profits
        function withdrawProfits(address token) external;
        
        /// Emergency stop
        function pause() external;
        
        /// Resume operations
        function unpause() external;
    }
    
    /// Uniswap V3 Router for swaps
    interface ISwapRouter {
        struct ExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint24 fee;
            address recipient;
            uint256 deadline;
            uint256 amountIn;
            uint256 amountOutMinimum;
            uint160 sqrtPriceLimitX96;
        }
        
        function exactInputSingle(ExactInputSingleParams calldata params)
            external payable returns (uint256 amountOut);
    }
    
    /// Uniswap V2 Router for swaps
    interface IUniswapV2Router {
        function swapExactTokensForTokens(
            uint256 amountIn,
            uint256 amountOutMin,
            address[] calldata path,
            address to,
            uint256 deadline
        ) external returns (uint256[] memory amounts);
    }
}

/// DEX type enum for the executor contract
#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub enum DexType {
    UniswapV3 = 0,
    UniswapV2 = 1,
    SushiswapV2 = 2,
    PancakeSwapV3 = 3,
    BalancerV2 = 4,
    Curve = 5,
}

impl From<Dex> for DexType {
    fn from(dex: Dex) -> Self {
        match dex {
            Dex::UniswapV3 | Dex::SushiswapV3 => DexType::UniswapV3,
            Dex::UniswapV2 => DexType::UniswapV2,
            Dex::SushiswapV2 => DexType::SushiswapV2,
            Dex::PancakeSwapV3 => DexType::PancakeSwapV3,
            Dex::BalancerV2 => DexType::BalancerV2,
            Dex::Curve => DexType::Curve,
        }
    }
}

// ============================================
// FLASH LOAN BUILDER
// ============================================

/// Builds and encodes flash loan transactions
pub struct FlashLoanBuilder {
    provider: FlashLoanProvider,
    executor_address: Option<Address>,
}

impl FlashLoanBuilder {
    pub fn new(config: &Config) -> Self {
        Self {
            provider: config.flash_loan_provider,
            executor_address: config
                .executor_contract_address
                .as_ref()
                .and_then(|s| s.parse().ok()),
        }
    }
    
    /// Build a flash loan transaction for the given arbitrage cycle
    pub fn build_flash_loan_tx(
        &self,
        cycle: &ArbitrageCycle,
        input_amount: U256,
        min_profit: U256,
    ) -> Result<FlashLoanTransaction> {
        let executor = self.executor_address
            .ok_or_else(|| eyre!("Executor contract address not configured"))?;
        
        // Get the starting token
        let start_token = cycle.path[0];
        
        // Build the arbitrage execution calldata
        let arb_calldata = self.build_arbitrage_calldata(cycle, input_amount, min_profit)?;
        
        // Build the flash loan request based on provider
        let (to, calldata) = match self.provider {
            FlashLoanProvider::BalancerV2 => {
                self.build_balancer_flash_loan(executor, start_token, input_amount, arb_calldata)?
            }
            FlashLoanProvider::AaveV3 => {
                self.build_aave_flash_loan(executor, start_token, input_amount, arb_calldata)?
            }
            FlashLoanProvider::UniswapV3 => {
                // For Uniswap, we don't use a separate flash loan
                // The executor handles flash swaps internally
                (executor, arb_calldata)
            }
        };
        
        Ok(FlashLoanTransaction {
            to,
            calldata,
            value: U256::ZERO,
            gas_limit: 1_000_000, // Will be estimated properly
            provider: self.provider,
        })
    }
    
    /// Build Balancer V2 flash loan calldata
    fn build_balancer_flash_loan(
        &self,
        recipient: Address,
        token: Address,
        amount: U256,
        user_data: Bytes,
    ) -> Result<(Address, Bytes)> {
        let call = IBalancerVault::flashLoanCall {
            recipient,
            tokens: vec![token],
            amounts: vec![amount],
            userData: user_data,
        };
        
        Ok((BALANCER_VAULT, Bytes::from(call.abi_encode())))
    }
    
    /// Build Aave V3 flash loan calldata
    fn build_aave_flash_loan(
        &self,
        recipient: Address,
        token: Address,
        amount: U256,
        params: Bytes,
    ) -> Result<(Address, Bytes)> {
        let call = IAavePool::flashLoanCall {
            receiverAddress: recipient,
            assets: vec![token],
            amounts: vec![amount],
            interestRateModes: vec![U256::ZERO], // No debt
            onBehalfOf: recipient,
            params,
            referralCode: 0,
        };
        
        Ok((AAVE_V3_POOL, Bytes::from(call.abi_encode())))
    }
    
    /// Build the arbitrage execution calldata
    fn build_arbitrage_calldata(
        &self,
        cycle: &ArbitrageCycle,
        input_amount: U256,
        min_output: U256,
    ) -> Result<Bytes> {
        // Convert cycle data to contract-compatible format
        let path: Vec<Address> = cycle.path.clone();
        let pools: Vec<Address> = cycle.pools.clone();
        let dex_types: Vec<u8> = cycle.dexes.iter().map(|d| DexType::from(*d) as u8).collect();
        
        let call = IArbitrageExecutor::executeArbitrageCall {
            path,
            pools,
            dexTypes: dex_types,
            inputAmount: input_amount,
            minOutput: min_output,
        };
        
        Ok(Bytes::from(call.abi_encode()))
    }
    
    /// Calculate the minimum output amount (input + min_profit - flash loan fee)
    pub fn calculate_min_output(
        &self,
        input_amount: U256,
        min_profit: U256,
    ) -> U256 {
        let fee = match self.provider {
            FlashLoanProvider::BalancerV2 => U256::ZERO, // 0% fee!
            FlashLoanProvider::AaveV3 => input_amount * U256::from(5) / U256::from(10000), // 0.05%
            FlashLoanProvider::UniswapV3 => input_amount * U256::from(3) / U256::from(1000), // ~0.3%
        };
        
        input_amount + min_profit + fee
    }
}

/// Represents a flash loan transaction ready for submission
#[derive(Debug, Clone)]
pub struct FlashLoanTransaction {
    pub to: Address,
    pub calldata: Bytes,
    pub value: U256,
    pub gas_limit: u64,
    pub provider: FlashLoanProvider,
}

impl FlashLoanTransaction {
    /// Estimate gas for this transaction
    pub async fn estimate_gas(&self, rpc_url: &str, from: Address) -> Result<u64> {
        let provider = ProviderBuilder::new()
            .on_http(rpc_url.parse()?);
        
        let tx = TransactionRequest::default()
            .from(from)
            .to(self.to)
            .input(self.calldata.clone().into())
            .value(self.value);
        
        let gas = provider.estimate_gas(tx).await
            .map_err(|e| eyre!("Gas estimation failed: {}", e))?;
        
        Ok(gas as u64)
    }
    
    /// Convert to a TransactionRequest for signing
    pub fn to_transaction_request(&self, from: Address, nonce: u64, gas_price: u128) -> TransactionRequest {
        TransactionRequest::default()
            .from(from)
            .to(self.to)
            .input(self.calldata.clone().into())
            .value(self.value)
            .nonce(nonce)
            .gas_limit(self.gas_limit)
            .max_fee_per_gas(gas_price)
            .max_priority_fee_per_gas(gas_price / 10) // 10% priority fee
    }
}

// ============================================
// EXECUTOR CONTRACT (Solidity Source)
// ============================================

/// Returns the Solidity source code for the executor contract
/// This needs to be compiled and deployed separately
pub fn get_executor_contract_source() -> &'static str {
    r#"
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import "@openzeppelin/contracts/security/ReentrancyGuard.sol";
import "@openzeppelin/contracts/access/Ownable.sol";
import "@openzeppelin/contracts/security/Pausable.sol";

// Flash loan receiver interfaces
interface IFlashLoanRecipient {
    function receiveFlashLoan(
        IERC20[] memory tokens,
        uint256[] memory amounts,
        uint256[] memory feeAmounts,
        bytes memory userData
    ) external;
}

interface IBalancerVault {
    function flashLoan(
        IFlashLoanRecipient recipient,
        IERC20[] memory tokens,
        uint256[] memory amounts,
        bytes memory userData
    ) external;
}

// DEX Router interfaces
interface IUniswapV3Router {
    struct ExactInputSingleParams {
        address tokenIn;
        address tokenOut;
        uint24 fee;
        address recipient;
        uint256 deadline;
        uint256 amountIn;
        uint256 amountOutMinimum;
        uint160 sqrtPriceLimitX96;
    }
    function exactInputSingle(ExactInputSingleParams calldata params) external payable returns (uint256);
}

interface IUniswapV2Router {
    function swapExactTokensForTokens(
        uint256 amountIn,
        uint256 amountOutMin,
        address[] calldata path,
        address to,
        uint256 deadline
    ) external returns (uint256[] memory amounts);
}

/**
 * @title ArbitrageExecutor
 * @notice Executes arbitrage opportunities via flash loans
 * @dev Uses Balancer V2 flash loans (0% fee) by default
 */
contract ArbitrageExecutor is IFlashLoanRecipient, Ownable, ReentrancyGuard, Pausable {
    using SafeERC20 for IERC20;
    
    // Constants
    IBalancerVault public constant BALANCER_VAULT = IBalancerVault(0xBA12222222228d8Ba445958a75a0704d566BF2C8);
    IUniswapV3Router public constant UNISWAP_V3_ROUTER = IUniswapV3Router(0xE592427A0AEce92De3Edee1F18E0157C05861564);
    IUniswapV2Router public constant UNISWAP_V2_ROUTER = IUniswapV2Router(0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D);
    IUniswapV2Router public constant SUSHISWAP_ROUTER = IUniswapV2Router(0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F);
    IUniswapV3Router public constant PANCAKE_V3_ROUTER = IUniswapV3Router(0x1b81D678ffb9C0263b24A97847620C99d213eB14);
    
    // DEX types
    uint8 constant DEX_UNISWAP_V3 = 0;
    uint8 constant DEX_UNISWAP_V2 = 1;
    uint8 constant DEX_SUSHISWAP_V2 = 2;
    uint8 constant DEX_PANCAKE_V3 = 3;
    uint8 constant DEX_BALANCER_V2 = 4;
    
    // Events
    event ArbitrageExecuted(address indexed token, uint256 inputAmount, uint256 profit);
    event ProfitWithdrawn(address indexed token, uint256 amount);
    
    // Profit tracking
    mapping(address => uint256) public accumulatedProfits;
    
    constructor() Ownable(msg.sender) {}
    
    /**
     * @notice Execute an arbitrage opportunity
     * @param path Token path for the arbitrage
     * @param pools Pool addresses for each swap
     * @param dexTypes DEX type for each swap
     * @param inputAmount Amount to borrow via flash loan
     * @param minOutput Minimum output required (input + fee + min profit)
     */
    function executeArbitrage(
        address[] calldata path,
        address[] calldata pools,
        uint8[] calldata dexTypes,
        uint256 inputAmount,
        uint256 minOutput
    ) external onlyOwner nonReentrant whenNotPaused returns (uint256 profit) {
        require(path.length >= 2, "Invalid path");
        require(path.length - 1 == pools.length, "Path/pools mismatch");
        require(pools.length == dexTypes.length, "Pools/dexTypes mismatch");
        
        // Encode arbitrage data for callback
        bytes memory userData = abi.encode(path, pools, dexTypes, minOutput);
        
        // Request flash loan
        IERC20[] memory tokens = new IERC20[](1);
        tokens[0] = IERC20(path[0]);
        
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = inputAmount;
        
        // Get initial balance
        uint256 balanceBefore = tokens[0].balanceOf(address(this));
        
        // Execute flash loan (callback will do the swaps)
        BALANCER_VAULT.flashLoan(this, tokens, amounts, userData);
        
        // Calculate profit
        uint256 balanceAfter = tokens[0].balanceOf(address(this));
        require(balanceAfter >= balanceBefore, "Arbitrage failed");
        
        profit = balanceAfter - balanceBefore;
        accumulatedProfits[path[0]] += profit;
        
        emit ArbitrageExecuted(path[0], inputAmount, profit);
        
        return profit;
    }
    
    /**
     * @notice Callback from Balancer flash loan
     */
    function receiveFlashLoan(
        IERC20[] memory tokens,
        uint256[] memory amounts,
        uint256[] memory feeAmounts,
        bytes memory userData
    ) external override {
        require(msg.sender == address(BALANCER_VAULT), "Only Balancer Vault");
        
        // Decode arbitrage parameters
        (
            address[] memory path,
            address[] memory pools,
            uint8[] memory dexTypes,
            uint256 minOutput
        ) = abi.decode(userData, (address[], address[], uint8[], uint256));
        
        // Execute the swap chain
        uint256 currentAmount = amounts[0];
        
        for (uint256 i = 0; i < pools.length; i++) {
            currentAmount = _executeSwap(
                path[i],
                path[i + 1],
                pools[i],
                dexTypes[i],
                currentAmount
            );
        }
        
        // Verify we have enough to repay + profit
        require(currentAmount >= minOutput, "Insufficient output");
        
        // Repay flash loan (Balancer has 0% fee!)
        uint256 amountOwed = amounts[0] + feeAmounts[0];
        tokens[0].safeTransfer(address(BALANCER_VAULT), amountOwed);
    }
    
    /**
     * @notice Execute a single swap on the specified DEX
     */
    function _executeSwap(
        address tokenIn,
        address tokenOut,
        address pool,
        uint8 dexType,
        uint256 amountIn
    ) internal returns (uint256 amountOut) {
        // Approve the router
        IERC20(tokenIn).safeApprove(_getRouter(dexType), amountIn);
        
        if (dexType == DEX_UNISWAP_V3 || dexType == DEX_PANCAKE_V3) {
            // V3 swap
            IUniswapV3Router router = dexType == DEX_UNISWAP_V3 
                ? UNISWAP_V3_ROUTER 
                : PANCAKE_V3_ROUTER;
            
            // Get fee tier from pool (simplified - in production query the pool)
            uint24 fee = 3000; // Default to 0.3%
            
            IUniswapV3Router.ExactInputSingleParams memory params = IUniswapV3Router.ExactInputSingleParams({
                tokenIn: tokenIn,
                tokenOut: tokenOut,
                fee: fee,
                recipient: address(this),
                deadline: block.timestamp,
                amountIn: amountIn,
                amountOutMinimum: 0, // We check final output
                sqrtPriceLimitX96: 0
            });
            
            amountOut = router.exactInputSingle(params);
        } else {
            // V2 swap
            IUniswapV2Router router = dexType == DEX_SUSHISWAP_V2 
                ? SUSHISWAP_ROUTER 
                : UNISWAP_V2_ROUTER;
            
            address[] memory swapPath = new address[](2);
            swapPath[0] = tokenIn;
            swapPath[1] = tokenOut;
            
            uint256[] memory amounts = router.swapExactTokensForTokens(
                amountIn,
                0, // We check final output
                swapPath,
                address(this),
                block.timestamp
            );
            
            amountOut = amounts[1];
        }
        
        // Reset approval
        IERC20(tokenIn).safeApprove(_getRouter(dexType), 0);
        
        return amountOut;
    }
    
    function _getRouter(uint8 dexType) internal pure returns (address) {
        if (dexType == DEX_UNISWAP_V3) return address(UNISWAP_V3_ROUTER);
        if (dexType == DEX_PANCAKE_V3) return address(PANCAKE_V3_ROUTER);
        if (dexType == DEX_SUSHISWAP_V2) return address(SUSHISWAP_ROUTER);
        return address(UNISWAP_V2_ROUTER);
    }
    
    /**
     * @notice Withdraw accumulated profits
     */
    function withdrawProfits(address token) external onlyOwner {
        uint256 amount = accumulatedProfits[token];
        require(amount > 0, "No profits");
        
        accumulatedProfits[token] = 0;
        IERC20(token).safeTransfer(owner(), amount);
        
        emit ProfitWithdrawn(token, amount);
    }
    
    /**
     * @notice Emergency withdraw any token
     */
    function emergencyWithdraw(address token) external onlyOwner {
        uint256 balance = IERC20(token).balanceOf(address(this));
        if (balance > 0) {
            IERC20(token).safeTransfer(owner(), balance);
        }
    }
    
    /**
     * @notice Pause the contract
     */
    function pause() external onlyOwner {
        _pause();
    }
    
    /**
     * @notice Unpause the contract
     */
    function unpause() external onlyOwner {
        _unpause();
    }
    
    // Receive ETH
    receive() external payable {}
}
"#
}

// ============================================
// TESTS
// ============================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartographer::Dex;
    
    #[test]
    fn test_dex_type_conversion() {
        assert_eq!(DexType::from(Dex::UniswapV3) as u8, 0);
        assert_eq!(DexType::from(Dex::UniswapV2) as u8, 1);
        assert_eq!(DexType::from(Dex::SushiswapV2) as u8, 2);
    }
    
    #[test]
    fn test_min_output_calculation() {
        let config = Config::default();
        let builder = FlashLoanBuilder::new(&config);
        
        let input = U256::from(1000u64);
        let min_profit = U256::from(10u64);
        
        // Balancer has 0% fee
        let min_output = builder.calculate_min_output(input, min_profit);
        assert_eq!(min_output, U256::from(1010u64));
    }
}
