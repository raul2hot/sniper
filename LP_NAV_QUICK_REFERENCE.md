# LP Token NAV Arbitrage - Quick Reference

## ⚠️ CRITICAL CONSTRAINTS

```
┌────────────────────────────────────────────────────────────────────┐
│  🔒 EXECUTOR CONTRACT IS SIGNED AND IMMUTABLE                       │
│                                                                     │
│  ❌ DO NOT create/modify any Solidity contracts                     │
│  ❌ DO NOT add new DEX types to executor                            │
│  ❌ DO NOT call add_liquidity or remove_liquidity                   │
│                                                                     │
│  ✅ DO trade LP tokens as ERC20s on UniV3/Balancer (already works)  │
│  ✅ DO use existing Dex::UniswapV3 and Dex::Curve types            │
│  ✅ DO implement all logic in Rust off-chain code                   │
└────────────────────────────────────────────────────────────────────┘
```

## Files to Create

```
src/cartographer/
├── mod.rs                          # MODIFY: Add `pub mod curve_lp;`
└── curve_lp/
    ├── mod.rs                      # NEW: Module exports
    ├── types.rs                    # NEW: Addresses, ABIs, constants
    ├── adapter.rs                  # NEW: CurveLPAdapter (pool discovery)
    ├── nav_calculator.rs           # NEW: NAV calculation + arb detection
    ├── market_discovery.rs         # NEW: Secondary market discovery
    └── tests.rs                    # NEW: Unit tests

src/cartographer/expanded_fetcher.rs  # MODIFY: Integrate LP components
```

## Module Exports (mod.rs)

```rust
// File: src/cartographer/curve_lp/mod.rs

mod types;
mod adapter;
mod nav_calculator;
mod market_discovery;

#[cfg(test)]
mod tests;

pub use types::*;
pub use adapter::{CurveLPAdapter, CachedLPPool};
pub use nav_calculator::{
    LPNavCalculator,
    LPNavResult,
    LPNavArbitrage,
    LPArbDirection,
};
pub use market_discovery::{
    LPMarketDiscovery,
    SecondaryMarket,
    SecondaryDex,
};
```

## RPC Call Budget

```
TARGET: <5 RPC calls per scan for LP system

Cold Start (Scan 1):
  └─ 2 multicalls: pool discovery + market discovery

Warm Cache (Scan 2-9):
  └─ 1 multicall: virtual price refresh only

Rediscovery (Scan 10):
  └─ 2 multicalls: same as cold start

Average: ~1.2 RPC calls per scan
```

## Cache Durations

| Data Type | Duration | Rationale |
|-----------|----------|-----------|
| Pool structure (coins, fees) | 5 min | Rarely changes |
| Secondary markets | 5 min | Pools don't appear/disappear often |
| virtual_price | 60 sec | Slow-moving (~8 bps/day) |
| UniV3 spot prices | 0 sec | Fetch during simulation |

## Key Contract Addresses

```rust
// Curve
CURVE_NG_FACTORY: 0x6A8cbed756804B16E05E741eDaBd5cB544AE21bf
CURVE_META_REGISTRY: 0xF98B45FA17DE75FB1aD0e7aFD971b0ca00e379fC

// High-TVL LP Tokens
3CRV: 0x6c3F90f043a72FA612cbac8115EE7e52BDe6E490
steCRV: 0x06325440D014e39736583c165C2963BA99fAf14E
crvFRAX: 0x3175Df0976dFA876431C2E9eE6Bc45b65d3473CC

// Secondary Markets
UNISWAP_V3_FACTORY: 0x1F98431c8aD98523631AE4a59f267346ea31F984
BALANCER_VAULT: 0xBA12222222228d8Ba445958a75a0704d566BF2C8

// Infrastructure
MULTICALL3: 0xcA11bde05977b3631167028862bE2a173976CA11
```

## Arbitrage Flow

```
┌─────────────────────────────────────────────────────────────┐
│                    LP NAV DISCOUNT ARB                       │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  DETECTION (off-chain):                                     │
│    1. Fetch virtual_price for LP tokens                     │
│    2. Calculate NAV = virtual_price × min(underlying_prices)│
│    3. Fetch LP spot price on UniV3 secondary market         │
│    4. If spot_price < NAV - 35bps → opportunity!            │
│                                                              │
│  EXECUTION (uses existing executor):                        │
│    1. Flash loan USDC from Balancer (0% fee)                │
│    2. Swap USDC → LP token on UniV3 (DEX_UNISWAP_V3=0)      │
│    3. Swap LP token → USDT on Curve (DEX_CURVE=5)           │
│    4. Swap USDT → USDC on UniV3 (DEX_UNISWAP_V3=0)          │
│    5. Repay flash loan + keep profit                        │
│                                                              │
│  WHY IT WORKS:                                              │
│    - LP tokens are ERC20s that trade on UniV3               │
│    - Executor already supports UniV3 swaps                  │
│    - No new contract functionality needed!                  │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

## Safety Checks (Mandatory)

```rust
// 1. virtual_price sanity check
fn validate_virtual_price(vp: U256) -> bool {
    let vp_f64 = vp.to::<u128>() as f64 / 1e18;
    vp_f64 >= 1.0 && vp_f64 <= 2.0
}

// 2. Secondary market liquidity check
const MIN_LIQUIDITY_USD: f64 = 50_000.0;
const MAX_TRADE_PCT: f64 = 0.10; // Don't trade >10% of liquidity

// 3. Profit threshold (gas buffer)
const MIN_PROFIT_BPS: u64 = 35; // 35 bps minimum after gas
```

## Integration Checklist

Before submitting PR, verify:

- [ ] No changes to `src/executor/` directory
- [ ] No new Solidity code anywhere
- [ ] All LP swaps use `Dex::UniswapV3` (existing executor support)
- [ ] Multicall3 used for all batch operations
- [ ] Cache durations match specification
- [ ] Discovery throttled every 10th scan
- [ ] virtual_price validation implemented
- [ ] Unit tests passing
- [ ] RPC call count verified (<5 per scan)

## Common Mistakes to Avoid

```rust
// ❌ WRONG: Individual RPC calls in a loop
for pool in pools {
    let vp = get_virtual_price(pool).await?; // N calls!
}

// ✅ RIGHT: Batched multicall
let calls = pools.iter().map(|p| Call3 { ... }).collect();
let results = multicall(calls).await?; // 1 call!

// ❌ WRONG: Using add_liquidity (executor doesn't support)
pool.add_liquidity([amount, 0], 0).call().await?;

// ✅ RIGHT: Buy LP token on secondary market
uniswap_v3_pool.swap(lp_token, usdc, amount).await?;

// ❌ WRONG: Adding new DEX type
pub enum DexType {
    CurveLPMint = 6, // DON'T DO THIS!
}

// ✅ RIGHT: Use existing DEX types
Dex::UniswapV3 // For LP token secondary markets
Dex::Curve     // For Curve pool exchanges
```

## Testing Commands

```bash
# Run LP-specific tests
cargo test curve_lp --lib

# Run with logging to verify RPC calls
RUST_LOG=debug cargo test curve_lp --lib 2>&1 | grep -i multicall

# Integration test (requires RPC)
RPC_URL=<your-url> cargo run --bin discover-pools
```
