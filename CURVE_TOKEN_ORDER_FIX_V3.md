# Curve Pool Token Order Bug - Final Fix V3

## Problem Summary

The `add_bridging_pools()` function has a **critical bug**: it assumes `token0` from `NewPoolInfo` is at Curve pool index 0, but Curve pools have their own coin ordering.

### Evidence from Logs

```
WARN Suspicious stablecoin price 2574036145610710.0000 for crvUSD -> USDT, skipping
```

Price is **2.5 quadrillion** instead of ~1.0!

---

## Root Cause: Token Index Mismatch

### Current Buggy Code

```rust
// In add_bridging_pools()
let forward_requests: Vec<(Address, i128, i128, U256)> = valid_pools.iter()
    .map(|(addr, info)| {
        let dx = U256::from((base_amount_usd * 10_f64.powi(info.token0_decimals as i32)) as u128);
        (*addr, 0i128, 1i128, dx)  // ← BUG: ALWAYS uses i=0, j=1
    })
    .collect();
```

### Why This Fails

For pool `0x390f3595bCa2Df7d23783dFd126427CCeb997BF4`:

| What code thinks | What pool actually has |
|------------------|------------------------|
| index 0 = crvUSD (18 dec) | index 0 = USDT (6 dec) |
| index 1 = USDT (6 dec) | index 1 = crvUSD (18 dec) |

**Calculation breakdown:**
1. Code calculates: `dx = 10000 * 10^18` (for "crvUSD" at 18 decimals)
2. Calls: `get_dy(pool, 0, 1, dx)` 
3. Pool sees index 0 = USDT, interprets `10^18` as `10^18 / 10^6 = 10^12` USDT
4. Returns: ~10^12 worth of crvUSD
5. Price = `dy / dx` without proper normalization = astronomical number

---

## Fix: Query Actual Token Indices

### Option A: Look Up Indices Dynamically (Recommended)

**File:** `src/cartographer/expanded_fetcher.rs`

```rust
/// Add bridging pools WITH actual on-chain prices (BATCHED)
/// FIXED: Now queries actual token indices from pool
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

    // Step 1: Query actual coin order from each pool
    let pool_coins = self.fetch_pool_coins(&valid_pools).await;
    
    // Step 2: Build requests with CORRECT indices
    let base_amount_usd = 10000.0;
    let mut forward_requests: Vec<(Address, i128, i128, U256)> = Vec::new();
    let mut request_metadata: Vec<(Address, &NewPoolInfo, u8, u8)> = Vec::new(); // (addr, info, actual_i_dec, actual_j_dec)
    
    for (pool_address, pool_info) in &valid_pools {
        // Find actual indices for our tokens
        let coins = match pool_coins.get(pool_address) {
            Some(c) => c,
            None => {
                warn!("Could not get coins for pool {:?}, skipping", pool_address);
                continue;
            }
        };
        
        // Find index of token0 in pool's coins array
        let i = coins.iter().position(|c| *c == pool_info.token0);
        let j = coins.iter().position(|c| *c == pool_info.token1);
        
        match (i, j) {
            (Some(i_idx), Some(j_idx)) => {
                // Get actual decimals for token at index i
                let dec_i = get_token_decimals(&coins[i_idx]);
                let dx = U256::from((base_amount_usd * 10_f64.powi(dec_i as i32)) as u128);
                
                forward_requests.push((*pool_address, i_idx as i128, j_idx as i128, dx));
                request_metadata.push((*pool_address, *pool_info, dec_i, get_token_decimals(&coins[j_idx])));
                
                debug!(
                    "Pool {:?}: {} at index {}, {} at index {}",
                    pool_address, pool_info.token0_symbol, i_idx, pool_info.token1_symbol, j_idx
                );
            }
            _ => {
                warn!(
                    "Tokens not found in pool {:?}: {} or {} not in {:?}",
                    pool_address, pool_info.token0_symbol, pool_info.token1_symbol, coins
                );
                continue;
            }
        }
    }

    if forward_requests.is_empty() {
        warn!("No valid forward requests after token index lookup");
        return 0;
    }

    // Step 3: Batch fetch prices
    let forward_prices = match self.curve_ng_fetcher.batch_get_dy(&forward_requests).await {
        Ok(results) => results,
        Err(e) => {
            warn!("Batch forward price fetch failed: {}", e);
            return 0;
        }
    };

    // Step 4: Create pool states with correct decimal normalization
    let mut count = 0;
    for (idx, price_result) in forward_prices.iter().enumerate() {
        let (pool_address, pool_info, dec_i, dec_j) = &request_metadata[idx];
        
        if let Some(dy) = price_result {
            let (_, _, _, dx) = forward_requests[idx];
            
            // Normalize using ACTUAL decimals from pool lookup
            let dx_normalized = dx.to::<u128>() as f64 / 10_f64.powi(*dec_i as i32);
            let dy_normalized = dy.to::<u128>() as f64 / 10_f64.powi(*dec_j as i32);
            
            if dx_normalized > 0.0 {
                let price = dy_normalized / dx_normalized;
                
                debug!(
                    "Pool {:?} ({}/{}): dx_norm={:.2}, dy_norm={:.2}, price={:.6}",
                    pool_address, pool_info.token0_symbol, pool_info.token1_symbol,
                    dx_normalized, dy_normalized, price
                );
                
                // Validate stablecoin prices
                if is_stablecoin_pair(pool_info) && (price < 0.8 || price > 1.25) {
                    warn!(
                        "Suspicious stablecoin price {:.4} for {} -> {}, skipping",
                        price, pool_info.token0_symbol, pool_info.token1_symbol
                    );
                    continue;
                }
                
                // Create PoolState with correct info
                let sqrt_price = price.sqrt() * 2_f64.powi(96);
                let pool_state = PoolState {
                    address: *pool_address,
                    token0: pool_info.token0,
                    token1: pool_info.token1,
                    token0_decimals: pool_info.token0_decimals,
                    token1_decimals: pool_info.token1_decimals,
                    sqrt_price_x96: U256::from(sqrt_price as u128),
                    tick: 0,
                    liquidity: 1_000_000_000_000_000_000u128,
                    reserve1: 1_000_000_000_000_000_000u128,
                    fee: pool_info.fee,
                    is_v4: false,
                    dex: pool_info.dex,
                    pool_type: pool_info.pool_type,
                    weight0: 5 * 10u128.pow(17),
                };
                
                pool_states.push(pool_state);
                count += 1;
            }
        } else {
            warn!("Could not fetch forward price for {:?}, skipping", pool_address);
        }
    }

    info!("Added {} bridging pools with accurate prices", count);
    count
}

/// Fetch coins array for each pool via Multicall
async fn fetch_pool_coins(
    &self,
    pools: &[(Address, &NewPoolInfo)]
) -> HashMap<Address, Vec<Address>> {
    use alloy_sol_types::SolCall;
    
    // Build multicall for coins(0), coins(1) for each pool
    let mut calls = Vec::new();
    
    for (pool_address, _) in pools {
        // Query coins(0) and coins(1)
        let call0 = ICurvePool::coinsCall { i: U256::from(0) };
        let call1 = ICurvePool::coinsCall { i: U256::from(1) };
        
        calls.push((*pool_address, call0.abi_encode()));
        calls.push((*pool_address, call1.abi_encode()));
    }
    
    // Execute multicall
    let results = match self.multicall(&calls).await {
        Ok(r) => r,
        Err(e) => {
            warn!("Failed to fetch pool coins: {}", e);
            return HashMap::new();
        }
    };
    
    // Parse results
    let mut pool_coins: HashMap<Address, Vec<Address>> = HashMap::new();
    
    for (i, (pool_address, _)) in pools.iter().enumerate() {
        let idx0 = i * 2;
        let idx1 = i * 2 + 1;
        
        if idx1 >= results.len() {
            continue;
        }
        
        let coin0 = if results[idx0].success {
            Address::from_slice(&results[idx0].returnData[12..32])
        } else {
            continue;
        };
        
        let coin1 = if results[idx1].success {
            Address::from_slice(&results[idx1].returnData[12..32])
        } else {
            continue;
        };
        
        pool_coins.insert(*pool_address, vec![coin0, coin1]);
    }
    
    pool_coins
}

fn is_stablecoin_pair(info: &NewPoolInfo) -> bool {
    let stables = ["USD", "DAI", "FRAX", "DOLA", "GHO", "crvUSD", "LUSD", "TUSD", "GUSD"];
    
    let sym0 = info.token0_symbol.to_uppercase();
    let sym1 = info.token1_symbol.to_uppercase();
    
    stables.iter().any(|s| sym0.contains(s)) && stables.iter().any(|s| sym1.contains(s))
}
```

---

### Option B: Store Correct Indices in NewPoolInfo (Simpler)

If you don't want to query on-chain, store the actual pool indices:

```rust
/// New pool info for static definition - WITH CORRECT POOL INDICES
#[derive(Debug, Clone)]
pub struct NewPoolInfo {
    pub address: &'static str,
    pub token0: Address,
    pub token1: Address,
    pub token0_symbol: &'static str,
    pub token1_symbol: &'static str,
    pub token0_decimals: u8,
    pub token1_decimals: u8,
    pub pool_index_0: i128,  // ADD: Actual index of token0 in pool.coins[]
    pub pool_index_1: i128,  // ADD: Actual index of token1 in pool.coins[]
    pub fee: u32,
    pub dex: Dex,
    pub pool_type: PoolType,
    pub note: &'static str,
}
```

Then verify each pool's coin order on-chain and update the definitions:

```rust
// crvUSD/USDT pool - VERIFIED coin order
// On-chain: coins[0] = USDT, coins[1] = crvUSD
NewPoolInfo {
    address: "0x390f3595bCa2Df7d23783dFd126427CCeb997BF4",
    token0: address!("f939E0A03FB07F59A73314E73794Be0E57ac1b4E"), // crvUSD
    token1: address!("dAC17F958D2ee523a2206206994597C13D831ec7"), // USDT
    token0_symbol: "crvUSD",
    token1_symbol: "USDT",
    token0_decimals: 18,
    token1_decimals: 6,
    pool_index_0: 1,  // crvUSD is at pool index 1
    pool_index_1: 0,  // USDT is at pool index 0
    fee: 4,
    dex: Dex::Curve,
    pool_type: PoolType::Curve,
    note: "Pegkeeper dynamics",
},
```

Then use these indices in the request:

```rust
let forward_requests: Vec<(Address, i128, i128, U256)> = valid_pools.iter()
    .map(|(addr, info)| {
        let dx = U256::from((base_amount_usd * 10_f64.powi(info.token0_decimals as i32)) as u128);
        (*addr, info.pool_index_0, info.pool_index_1, dx)  // Use stored indices!
    })
    .collect();
```

---

## How to Verify Pool Coin Order

Use cast or ethers to check:

```bash
# Check crvUSD/USDT pool coin order
cast call 0x390f3595bCa2Df7d23783dFd126427CCeb997BF4 "coins(uint256)(address)" 0
# Returns: 0xdAC17F958D2ee523a2206206994597C13D831ec7 (USDT)

cast call 0x390f3595bCa2Df7d23783dFd126427CCeb997BF4 "coins(uint256)(address)" 1  
# Returns: 0xf939E0A03FB07F59A73314E73794Be0E57ac1b4E (crvUSD)
```

So for this pool:
- **coins[0] = USDT** (not crvUSD!)
- **coins[1] = crvUSD**

---

## Pool Index Reference Table

Here are the verified indices for common Curve pools:

| Pool | Address | coins[0] | coins[1] |
|------|---------|----------|----------|
| crvUSD/USDT | 0x390f3595... | USDT | crvUSD |
| crvUSD/USDC | 0x4DEcE678... | USDC | crvUSD |
| FRAX/USDC | 0xDcEF968d... | FRAX | USDC |
| DOLA/USDC | 0xAA5A67c2... | DOLA | USDC |
| DAI/USDS | 0x3225737a... | DAI | USDS |

**IMPORTANT:** Always verify on-chain before adding pools!

---

## Complete Fixed `get_new_priority_pools()` with Correct Indices

```rust
fn get_new_priority_pools() -> Vec<NewPoolInfo> {
    vec![
        // crvUSD/USDT - VERIFIED: coins[0]=USDT, coins[1]=crvUSD
        NewPoolInfo {
            address: "0x390f3595bCa2Df7d23783dFd126427CCeb997BF4",
            token0: address!("f939E0A03FB07F59A73314E73794Be0E57ac1b4E"), // crvUSD
            token1: address!("dAC17F958D2ee523a2206206994597C13D831ec7"), // USDT
            token0_symbol: "crvUSD",
            token1_symbol: "USDT",
            token0_decimals: 18,
            token1_decimals: 6,
            pool_index_0: 1,  // crvUSD at index 1
            pool_index_1: 0,  // USDT at index 0
            fee: 4,
            dex: Dex::Curve,
            pool_type: PoolType::Curve,
            note: "Pegkeeper dynamics",
        },
        
        // crvUSD/USDC - VERIFY ON-CHAIN BEFORE USING
        NewPoolInfo {
            address: "0x4DEcE678ceceb27446b35C672dC7d61F30bAD69E",
            token0: address!("f939E0A03FB07F59A73314E73794Be0E57ac1b4E"), // crvUSD
            token1: address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"), // USDC
            token0_symbol: "crvUSD",
            token1_symbol: "USDC",
            token0_decimals: 18,
            token1_decimals: 6,
            pool_index_0: 1,  // crvUSD at index 1 (VERIFY!)
            pool_index_1: 0,  // USDC at index 0 (VERIFY!)
            fee: 4,
            dex: Dex::Curve,
            pool_type: PoolType::Curve,
            note: "High volume crvUSD pool",
        },
        
        // ... add more with verified indices
    ]
}
```

---

## Verification After Fix

After applying the fix, you should see:

```
DEBUG Pool 0x390f3595...: crvUSD at index 1, USDT at index 0
DEBUG Pool 0x390f3595... (crvUSD/USDT): dx_norm=10000.00, dy_norm=9998.50, price=0.999850
✅ OK: crvUSD → USDT price = 0.9999
```

Instead of:
```
WARN Suspicious stablecoin price 2574036145610710.0000 for crvUSD -> USDT, skipping
```

---

## Summary of Changes

| File | Change | Description |
|------|--------|-------------|
| `expanded_fetcher.rs` | Add `fetch_pool_coins()` | Query actual token order from pool |
| `expanded_fetcher.rs` | Fix `add_bridging_pools()` | Use correct indices from lookup |
| OR: `NewPoolInfo` struct | Add `pool_index_0`, `pool_index_1` | Store verified indices |
| Pool definitions | Update all pools | Add correct indices based on on-chain verification |

---

## Executor Safety

✅ **All changes are off-chain Rust code only**
- Pool discovery and price calculation
- No changes to smart contracts
- No changes to executor logic
- The executor will continue to work exactly as before
