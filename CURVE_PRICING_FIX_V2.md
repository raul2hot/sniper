# Curve Pool Pricing Fix V2 - Remaining Bugs

## Current Status: NOT FIXED

The previous fix was **partially applied**, but there are still **critical bugs** causing false positives:

```
Step 2: USDT → crvUSD
       price=38853.32336700   ← WRONG! Should be ~1.0
```

This price of 38,853 is clearly wrong (both are ~$1 stablecoins).

---

## Root Cause: Decimal Mismatch in `add_bridging_pools()`

### Bug Location: `src/cartographer/expanded_fetcher.rs`

```rust
async fn add_bridging_pools(&self, pool_states: &mut Vec<PoolState>) -> usize {
    // ...
    
    // BUG #1: Hardcoded 18 decimals for ALL tokens
    let dx = U256::from(10u64.pow(18));  // ← WRONG for USDT (6 decimals)
    let requests: Vec<(Address, i128, i128, U256)> = valid_pools.iter()
        .map(|(addr, _)| (*addr, 0i128, 1i128, dx))
        .collect();
    
    // ...
    
    // BUG #2: No decimal adjustment in price calculation
    let price = match price_result {
        Some(dy) => {
            let p = dy.to::<u128>() as f64 / dx.to::<u128>() as f64;  // ← WRONG!
            if p > 0.0 && p.is_finite() { p } else { 1.0 }
        }
        // ...
    };
}
```

### Why This Causes 38,853x Price

For USDT (6 decimals) → crvUSD (18 decimals):

1. Code sends `dx = 10^18` to `get_dy`
2. But USDT only has 6 decimals, so this represents `10^18 / 10^6 = 10^12` USDT ($1 trillion!)
3. Pool returns `dy` in crvUSD (18 decimals)
4. Price calculation: `dy / dx` without decimal normalization
5. The decimal difference (18-6=12) causes a `10^12` factor error

**Math:**
- Real: 1 USDT ≈ 1 crvUSD
- Buggy calculation: `price = (dy * 10^18) / (10^18)` but dy is based on 10^12 USDT input
- This creates a ~10^12 / 10^12 = massive distortion in the ratio

### Bug #3: `NewPoolInfo` Missing Decimals

```rust
pub struct NewPoolInfo {
    pub address: &'static str,
    pub token0: Address,
    pub token1: Address,
    pub token0_symbol: &'static str,
    pub token1_symbol: &'static str,
    pub fee: u32,
    pub dex: Dex,
    pub pool_type: PoolType,
    pub note: &'static str,
    // ❌ NO DECIMALS STORED!
}
```

---

## Fix Implementation

### Fix 1: Add Decimals to `NewPoolInfo`

**File:** `src/cartographer/expanded_fetcher.rs`

```rust
/// New pool info for static definition - NOW WITH DECIMALS
#[derive(Debug, Clone)]
pub struct NewPoolInfo {
    pub address: &'static str,
    pub token0: Address,
    pub token1: Address,
    pub token0_symbol: &'static str,
    pub token1_symbol: &'static str,
    pub token0_decimals: u8,  // ADD THIS
    pub token1_decimals: u8,  // ADD THIS
    pub fee: u32,
    pub dex: Dex,
    pub pool_type: PoolType,
    pub note: &'static str,
}
```

### Fix 2: Update All Pool Definitions

**File:** `src/cartographer/expanded_fetcher.rs` - `get_new_priority_pools()`

```rust
fn get_new_priority_pools() -> Vec<NewPoolInfo> {
    vec![
        // crvUSD/USDT - high volume
        NewPoolInfo {
            address: "0x390f3595bCa2Df7d23783dFd126427CCeb997BF4",
            token0: address!("f939E0A03FB07F59A73314E73794Be0E57ac1b4E"), // crvUSD
            token1: address!("dAC17F958D2ee523a2206206994597C13D831ec7"), // USDT
            token0_symbol: "crvUSD",
            token1_symbol: "USDT",
            token0_decimals: 18,  // crvUSD = 18 decimals
            token1_decimals: 6,   // USDT = 6 decimals
            fee: 4,
            dex: Dex::Curve,
            pool_type: PoolType::Curve,
            note: "Pegkeeper dynamics create spreads",
        },
        
        // crvUSD/USDC
        NewPoolInfo {
            address: "0x4DEcE678ceceb27446b35C672dC7d61F30bAD69E",
            token0: address!("f939E0A03FB07F59A73314E73794Be0E57ac1b4E"), // crvUSD
            token1: address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"), // USDC
            token0_symbol: "crvUSD",
            token1_symbol: "USDC",
            token0_decimals: 18,  // crvUSD = 18 decimals
            token1_decimals: 6,   // USDC = 6 decimals
            fee: 4,
            dex: Dex::Curve,
            pool_type: PoolType::Curve,
            note: "High volume crvUSD pool",
        },
        
        // USDS/DAI - Sky migration pool
        NewPoolInfo {
            address: "0x3225737a9Bbb6473CB4a45b7244ACa2BeFdB276A",
            token0: USDS_TOKEN,
            token1: DAI_TOKEN,
            token0_symbol: "USDS",
            token1_symbol: "DAI",
            token0_decimals: 18,  // USDS = 18 decimals
            token1_decimals: 18,  // DAI = 18 decimals
            fee: 1,
            dex: Dex::Curve,
            pool_type: PoolType::Curve,
            note: "DAI-USDS 1:1 migration bridge",
        },
        
        // FRAX/USDC
        NewPoolInfo {
            address: "0xDcEF968d416a41Cdac0ED8702fAC8128A64241A2",
            token0: address!("853d955aCEf822Db058eb8505911ED77F175b99e"), // FRAX
            token1: address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"), // USDC
            token0_symbol: "FRAX",
            token1_symbol: "USDC",
            token0_decimals: 18,  // FRAX = 18 decimals
            token1_decimals: 6,   // USDC = 6 decimals
            fee: 4,
            dex: Dex::Curve,
            pool_type: PoolType::Curve,
            note: "FRAX peg maintenance creates opportunities",
        },
        
        // DOLA/USDC
        NewPoolInfo {
            address: "0xAA5A67c256e27A5d80712c51971408db3370927D",
            token0: address!("865377367054516e17014CcdED1e7d814EDC9ce4"), // DOLA
            token1: address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"), // USDC
            token0_symbol: "DOLA",
            token1_symbol: "USDC",
            token0_decimals: 18,  // DOLA = 18 decimals
            token1_decimals: 6,   // USDC = 6 decimals
            fee: 4,
            dex: Dex::Curve,
            pool_type: PoolType::Curve,
            note: "Leverage demand creates spreads",
        },
        
        // ... update ALL other pools similarly
    ]
}
```

### Fix 3: Fix `add_bridging_pools()` with Proper Decimal Handling

**File:** `src/cartographer/expanded_fetcher.rs`

```rust
/// Add bridging pools WITH actual on-chain prices (BATCHED)
/// Uses Multicall3 to fetch all prices in a single RPC call
/// FIXED: Now properly handles token decimals
async fn add_bridging_pools(&self, pool_states: &mut Vec<PoolState>) -> usize {
    let priority_pools = get_new_priority_pools();
    let mut valid_pools: Vec<(Address, &NewPoolInfo)> = Vec::new();

    for pool_info in &priority_pools {
        if pool_info.address.starts_with("0x...") || pool_info.address.len() < 20 {
            continue;
        }
        if let Ok(pool_address) = pool_info.address.parse::<Address>() {
            valid_pools.push((pool_address, pool_info));
        }
    }

    if valid_pools.is_empty() {
        return 0;
    }

    // FIX: Build batch request with CORRECT decimals for each token
    let base_amount_usd = 10000.0; // $10,000 worth
    let requests: Vec<(Address, i128, i128, U256)> = valid_pools.iter()
        .map(|(addr, info)| {
            // Use token0's decimals for the input amount
            let dx = U256::from((base_amount_usd * 10_f64.powi(info.token0_decimals as i32)) as u128);
            (*addr, 0i128, 1i128, dx)
        })
        .collect();

    // Batch fetch all prices in 1 RPC call
    let price_results = match self.curve_ng_fetcher.batch_get_dy(&requests).await {
        Ok(results) => results,
        Err(e) => {
            warn!("Batch price fetch failed: {}, skipping bridging pools", e);
            return 0;
        }
    };

    // Create pool states with fetched prices - WITH DECIMAL NORMALIZATION
    let mut count = 0;
    for ((pool_address, pool_info), price_result) in valid_pools.iter().zip(price_results.iter()) {
        let price = match price_result {
            Some(dy) => {
                let (_, _, _, dx) = requests[count];
                
                // FIX: Normalize by decimals
                let dx_normalized = dx.to::<u128>() as f64 / 10_f64.powi(pool_info.token0_decimals as i32);
                let dy_normalized = dy.to::<u128>() as f64 / 10_f64.powi(pool_info.token1_decimals as i32);
                
                if dx_normalized > 0.0 {
                    let p = dy_normalized / dx_normalized;
                    debug!(
                        "Pool {:?} ({}/{}): dx={}, dy={}, price={:.6}",
                        pool_address, pool_info.token0_symbol, pool_info.token1_symbol,
                        dx_normalized, dy_normalized, p
                    );
                    if p > 0.0 && p.is_finite() { p } else { continue }
                } else {
                    continue
                }
            }
            None => {
                warn!("Could not fetch price for {:?}, skipping", pool_address);
                continue;
            }
        };

        // FIX: Validate stablecoin prices are sane
        if pool_info.token0_symbol.contains("USD") || pool_info.token1_symbol.contains("USD") ||
           pool_info.token0_symbol.contains("DAI") || pool_info.token1_symbol.contains("DAI") ||
           pool_info.token0_symbol.contains("FRAX") || pool_info.token1_symbol.contains("FRAX") {
            // Both are stablecoins - price should be 0.8 - 1.25
            if price < 0.8 || price > 1.25 {
                warn!(
                    "Suspicious stablecoin price {:.4} for {} → {}, skipping",
                    price, pool_info.token0_symbol, pool_info.token1_symbol
                );
                continue;
            }
        }

        // Convert price to sqrt_price_x96 format
        let sqrt_price = price.sqrt() * 2_f64.powi(96);

        let pool_state = PoolState {
            address: *pool_address,
            token0: pool_info.token0,
            token1: pool_info.token1,
            token0_decimals: pool_info.token0_decimals,
            token1_decimals: pool_info.token1_decimals,
            sqrt_price_x96: U256::from(sqrt_price as u128),
            tick: 0,
            liquidity: 1_000_000_000_000_000_000u128, // Placeholder
            reserve1: 1_000_000_000_000_000_000u128,  // Placeholder
            fee: pool_info.fee,
            is_v4: false,
            dex: pool_info.dex,
            pool_type: pool_info.pool_type,
            weight0: 5 * 10u128.pow(17),
        };

        pool_states.push(pool_state);
        count += 1;
    }

    info!("Added {} bridging pools with accurate prices", count);
    count
}
```

### Fix 4: Add Reverse Direction Edges

The current code only adds token0 → token1 edges. For proper arbitrage detection, we need BOTH directions:

```rust
async fn add_bridging_pools(&self, pool_states: &mut Vec<PoolState>) -> usize {
    // ... existing code to get forward prices ...
    
    // ALSO fetch reverse prices (token1 → token0)
    let reverse_requests: Vec<(Address, i128, i128, U256)> = valid_pools.iter()
        .map(|(addr, info)| {
            let dx = U256::from((base_amount_usd * 10_f64.powi(info.token1_decimals as i32)) as u128);
            (*addr, 1i128, 0i128, dx)  // Reversed: j=1, i=0
        })
        .collect();

    let reverse_prices = self.curve_ng_fetcher.batch_get_dy(&reverse_requests).await
        .unwrap_or_else(|_| vec![None; valid_pools.len()]);

    // Add both forward AND reverse edges
    for ((pool_address, pool_info), (fwd_price, rev_price)) in 
        valid_pools.iter().zip(price_results.iter().zip(reverse_prices.iter())) 
    {
        // Add forward edge: token0 → token1
        if let Some(dy) = fwd_price {
            // ... create forward pool_state ...
            pool_states.push(forward_state);
        }

        // Add reverse edge: token1 → token0
        if let Some(dy) = rev_price {
            let (_, _, _, dx) = reverse_requests[count];
            let dx_normalized = dx.to::<u128>() as f64 / 10_f64.powi(pool_info.token1_decimals as i32);
            let dy_normalized = dy.to::<u128>() as f64 / 10_f64.powi(pool_info.token0_decimals as i32);
            
            if dx_normalized > 0.0 {
                let price = dy_normalized / dx_normalized;
                
                // Create reverse edge (swap token0/token1)
                let reverse_state = PoolState {
                    address: *pool_address,
                    token0: pool_info.token1,  // SWAPPED
                    token1: pool_info.token0,  // SWAPPED
                    token0_decimals: pool_info.token1_decimals,  // SWAPPED
                    token1_decimals: pool_info.token0_decimals,  // SWAPPED
                    sqrt_price_x96: U256::from((price.sqrt() * 2_f64.powi(96)) as u128),
                    // ... rest same ...
                };
                pool_states.push(reverse_state);
            }
        }
    }
}
```

---

## Alternative: Use `get_token_decimals()` Helper

Instead of storing decimals in `NewPoolInfo`, you can use the existing helper:

**File:** `src/cartographer/fetcher.rs`

```rust
pub fn get_token_decimals(address: &Address) -> u8 {
    let a = format!("{:?}", address).to_lowercase();

    // 6 decimals (stablecoins)
    if a.contains("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48")  // USDC
        || a.contains("dac17f958d2ee523a2206206994597c13d831ec7")  // USDT
        || a.contains("6c3ea9036406852006290770bedfcaba0e23a0e8")  // pyUSD
    {
        return 6;
    }
    // ... more cases ...
    18  // Default
}
```

Then in `add_bridging_pools()`:

```rust
let requests: Vec<(Address, i128, i128, U256)> = valid_pools.iter()
    .map(|(addr, info)| {
        // FIX: Get decimals from helper function
        let dec0 = get_token_decimals(&info.token0);
        let dx = U256::from((base_amount_usd * 10_f64.powi(dec0 as i32)) as u128);
        (*addr, 0i128, 1i128, dx)
    })
    .collect();
```

---

## Executor Safety: NO CHANGES REQUIRED

**CRITICAL:** The executor contract is already signed and deployed. These changes are ONLY in:

1. **Pool discovery** (`expanded_fetcher.rs`) - Rust code, not Solidity
2. **Price calculation** - Rust code for graph building
3. **Cycle detection** - Rust code in `brain/`

The fixes do **NOT** touch:
- ❌ `contracts/` directory
- ❌ `src/executor/` (flash loan execution)
- ❌ Any deployed smart contracts
- ❌ Flashbots bundle submission

The executor will continue to work exactly as before. We're only fixing the **off-chain** price discovery that feeds into cycle detection.

---

## Quick Verification Test

After applying fixes, run this validation:

```rust
// In main.rs or a test file
fn validate_curve_prices(pools: &[PoolState]) {
    let stablecoins = ["USDC", "USDT", "DAI", "crvUSD", "FRAX", "DOLA", "GHO", "USDS", "USDe"];
    
    for pool in pools {
        if pool.dex != Dex::Curve { continue; }
        
        let price = pool.normalized_price();
        let sym0 = get_symbol(&pool.token0);
        let sym1 = get_symbol(&pool.token1);
        
        // Check if both are stablecoins
        let both_stable = stablecoins.iter().any(|s| sym0.contains(s)) &&
                          stablecoins.iter().any(|s| sym1.contains(s));
        
        if both_stable {
            if price < 0.8 || price > 1.25 {
                println!("⚠️  SUSPICIOUS: {} → {} price = {:.4} (expected ~1.0)", 
                         sym0, sym1, price);
            } else {
                println!("✅ OK: {} → {} price = {:.4}", sym0, sym1, price);
            }
        }
    }
}
```

Expected output after fix:
```
✅ OK: crvUSD → USDT price = 0.9998
✅ OK: USDT → crvUSD price = 1.0002
✅ OK: crvUSD → USDC price = 0.9995
✅ OK: USDC → crvUSD price = 1.0005
```

---

## Summary of Changes

| File | Change | Risk |
|------|--------|------|
| `src/cartographer/expanded_fetcher.rs` | Add decimals to `NewPoolInfo` | None - struct change |
| `src/cartographer/expanded_fetcher.rs` | Fix `add_bridging_pools()` decimal handling | None - price calc only |
| `src/cartographer/expanded_fetcher.rs` | Add stablecoin price validation | None - filtering only |
| `get_new_priority_pools()` | Add `token0_decimals`, `token1_decimals` to each pool | None - data change |

**Executor impact:** ZERO - these are all off-chain Rust changes for price discovery.

---

## Files NOT to Modify

- ❌ `contracts/` - Executor is immutable
- ❌ `src/executor/flash_loan.rs` - Already working
- ❌ `src/executor/mod.rs` - Already working
- ❌ Any `.sol` files
- ❌ Flashbots integration code
