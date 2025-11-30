# Curve Pool Pricing Fix - Detailed Instructions for Claude Code

## Executive Summary

The arbitrage bot is detecting **false positive opportunities** showing 57%+ returns on stablecoin cycles. The root cause is that Curve pool prices are calculated from **balance ratios** instead of actual **on-chain exchange rates** (`get_dy`).

**Example of the bug:**
```
Cycle #0 (return: 1.575534)   ← FAKE - should be ~0.998
  Step 1: USDC → USDe
         price=1.39462969     ← WRONG! Real price is ~1.0
```

---

## Root Cause Analysis

### Problem 1: Balance-Ratio Pricing for Curve Pools

**File:** `src/cartographer/curve_ng.rs`  
**Method:** `CurveNGPool::to_pool_state()`

```rust
// CURRENT (BROKEN) CODE:
pub fn to_pool_state(&self, token0_idx: usize, token1_idx: usize) -> Option<PoolState> {
    // ...
    let bal0 = self.balances[token0_idx].to::<u128>() as f64;
    let bal1 = self.balances[token1_idx].to::<u128>() as f64;
    
    // THIS IS WRONG FOR CURVE POOLS!
    let price_raw = (bal1 / 10_f64.powi(d1 as i32)) / (bal0 / 10_f64.powi(d0 as i32));
    // ...
}
```

**Why it's wrong:** Curve StableSwap AMM uses a complex invariant with an amplification factor (`A`). The price is NOT `balance1/balance0`. A pool can have 70% token A and 30% token B but still price them at 1:1 due to the curve shape.

### Problem 2: Graph vs Simulator Price Source Mismatch

| Component | Price Source | Accuracy |
|-----------|--------------|----------|
| Graph Construction (`ArbitrageGraph::from_pools`) | Balance ratios | ❌ Wrong for Curve |
| Simulator (`SwapSimulator`) | On-chain quoter | ✅ Correct |

This means cycles are detected with phantom profits, but simulation would show different results.

### Problem 3: Curve Pools ARE Handled Specially, But Incorrectly

The code recognizes Curve pools need special handling:
```rust
pool_type: PoolType::Curve,
dex: Dex::Curve,
```

But `PoolState::normalized_price()` in `src/cartographer/fetcher.rs` uses the same V2-style formula for Curve:
```rust
_ => {
    // This is used for BOTH V2 AND Curve - wrong for Curve!
    if self.liquidity == 0 || self.reserve1 == 0 { return 0.0; }
    let price = (self.reserve1 as f64 / self.liquidity as f64)
        * 10_f64.powi(self.token0_decimals as i32 - self.token1_decimals as i32);
    // ...
}
```

---

## Fix Strategy Overview

There are two possible approaches:

### Option A: On-Chain Price Queries During Pool Discovery (Recommended)
- Query `get_dy` for each Curve pool during discovery
- Store accurate prices in `PoolState`
- Pro: Accurate prices, minimal code changes
- Con: Additional RPC calls (but can be batched with Multicall3)

### Option B: Separate Price Oracle for Curve Pools
- Keep balance-ratio for quick filtering
- Use `get_dy` during cycle validation before simulation
- Pro: Faster initial scan
- Con: More complex architecture

**Recommendation:** Go with **Option A** - accuracy matters more than speed for avoiding false positives.

---

## Detailed Implementation Steps

### Step 1: Add `get_dy` Price Fetching to Curve Pool Discovery

**File:** `src/cartographer/curve_ng.rs`

#### 1.1 Add new method to `CurveNGFetcher` for batch price queries

```rust
/// Batch fetch accurate prices using get_dy for all pool pairs
/// Returns HashMap<(pool_address, i, j), price_float>
pub async fn batch_fetch_prices(
    &self,
    pools: &[CurveNGPool],
    base_amount_usd: f64,  // e.g., 10000.0
) -> Result<HashMap<(Address, usize, usize), f64>> {
    let mut requests = Vec::new();
    let mut request_map = Vec::new(); // Track which request maps to which pool/pair
    
    for pool in pools {
        for i in 0..pool.n_coins {
            for j in 0..pool.n_coins {
                if i == j { continue; }
                
                // Calculate input amount based on token decimals
                let decimals = pool.decimals[i];
                // For stablecoins, use ~$10000 worth
                let dx = U256::from((base_amount_usd * 10_f64.powi(decimals as i32)) as u128);
                
                requests.push((pool.address, i as i128, j as i128, dx));
                request_map.push((pool.address, i, j, decimals, pool.decimals[j]));
            }
        }
    }
    
    // Use existing batch_get_dy method
    let results = self.batch_get_dy(&requests).await?;
    
    let mut prices = HashMap::new();
    for (idx, dy_opt) in results.into_iter().enumerate() {
        if let Some(dy) = dy_opt {
            let (pool_addr, i, j, dec_i, dec_j) = request_map[idx];
            let (_, _, _, dx) = requests[idx];
            
            // Calculate price: dy/dx with decimal adjustment
            let dx_f64 = dx.to::<u128>() as f64 / 10_f64.powi(dec_i as i32);
            let dy_f64 = dy.to::<u128>() as f64 / 10_f64.powi(dec_j as i32);
            
            if dx_f64 > 0.0 {
                let price = dy_f64 / dx_f64;
                prices.insert((pool_addr, i, j), price);
            }
        }
    }
    
    Ok(prices)
}
```

#### 1.2 Modify `to_pool_state()` to Accept Pre-fetched Price

```rust
/// Convert to standard PoolState for graph integration
/// NOW REQUIRES actual price from get_dy query
pub fn to_pool_state_with_price(
    &self, 
    token0_idx: usize, 
    token1_idx: usize,
    actual_price: f64,  // Price from get_dy: 1 token0 = X token1
) -> Option<PoolState> {
    if token0_idx >= self.coins.len() || token1_idx >= self.coins.len() {
        return None;
    }
    
    let token0 = self.coins[token0_idx];
    let token1 = self.coins[token1_idx];
    
    if actual_price <= 0.0 || !actual_price.is_finite() {
        return None;
    }
    
    let d0 = self.decimals[token0_idx];
    let d1 = self.decimals[token1_idx];
    let fee = self.effective_fee(token0_idx, token1_idx);
    
    // Convert price to sqrt_price_x96 format for consistency
    // Note: This is for storage only - actual Curve quotes should still use get_dy
    let sqrt_price = actual_price.sqrt() * 2_f64.powi(96);
    
    Some(PoolState {
        address: self.address,
        token0,
        token1,
        token0_decimals: d0,
        token1_decimals: d1,
        sqrt_price_x96: U256::from(sqrt_price as u128),
        tick: 0,
        liquidity: self.balances[token0_idx].to::<u128>(),
        reserve1: self.balances[token1_idx].to::<u128>(),
        fee,
        is_v4: false,
        dex: Dex::Curve,
        pool_type: PoolType::Curve,
        weight0: 5 * 10u128.pow(17),
    })
}
```

#### 1.3 Deprecate Old `to_pool_state()` Method

```rust
/// DEPRECATED: Use to_pool_state_with_price() instead
/// This method uses balance ratios which are INACCURATE for Curve pools
#[deprecated(note = "Use to_pool_state_with_price() with actual get_dy price")]
pub fn to_pool_state(&self, token0_idx: usize, token1_idx: usize) -> Option<PoolState> {
    // ... keep old code but mark as deprecated
}
```

---

### Step 2: Update Curve NG Pool Conversion to Use Accurate Prices

**File:** `src/cartographer/curve_ng.rs`

#### 2.1 Modify `ng_pools_to_pool_states()` to Fetch Prices First

```rust
/// Convert discovered NG pools to PoolState format for the routing graph
/// NOW FETCHES ACCURATE PRICES via get_dy
pub async fn ng_pools_to_pool_states_accurate(&self, ng_pools: &[CurveNGPool]) -> Vec<PoolState> {
    // Step 1: Batch fetch all prices
    let prices = match self.batch_fetch_prices(ng_pools, 10000.0).await {
        Ok(p) => p,
        Err(e) => {
            warn!("Failed to fetch Curve prices: {}, falling back to balance ratios", e);
            return self.ng_pools_to_pool_states(ng_pools); // Fallback to old method
        }
    };
    
    let mut states = Vec::new();
    
    for pool in ng_pools {
        for i in 0..pool.n_coins {
            for j in 0..pool.n_coins {
                if i == j { continue; }
                
                // Skip invalid addresses
                if !Self::is_valid_address(&pool.coins[i]) || 
                   !Self::is_valid_address(&pool.coins[j]) {
                    continue;
                }
                
                // Get pre-fetched price
                if let Some(&price) = prices.get(&(pool.address, i, j)) {
                    if let Some(state) = pool.to_pool_state_with_price(i, j, price) {
                        states.push(state);
                    }
                }
            }
        }
    }
    
    debug!("Converted {} NG pools to {} graph edges with accurate prices", 
           ng_pools.len(), states.len());
    states
}
```

---

### Step 3: Update ExpandedPoolFetcher to Use Accurate Prices

**File:** `src/cartographer/expanded_fetcher.rs`

Find where `ng_pools_to_pool_states()` is called and replace with the new async version:

```rust
// BEFORE:
let ng_pool_states = self.curve_ng_fetcher.ng_pools_to_pool_states(&ng_pools);

// AFTER:
let ng_pool_states = self.curve_ng_fetcher.ng_pools_to_pool_states_accurate(&ng_pools).await;
```

---

### Step 4: Add Price Validation Filter

**File:** `src/cartographer/expanded_fetcher.rs`

Add a sanity check for stablecoin prices:

```rust
/// Validate that stablecoin prices are sane (within 20% of $1)
fn validate_stablecoin_prices(pools: &mut Vec<PoolState>, stablecoins: &HashSet<Address>) {
    pools.retain(|pool| {
        let t0_stable = stablecoins.contains(&pool.token0);
        let t1_stable = stablecoins.contains(&pool.token1);
        
        if t0_stable && t1_stable {
            // Both stablecoins: price should be 0.8 - 1.2
            let price = pool.normalized_price();
            if price < 0.8 || price > 1.2 {
                warn!(
                    "Filtering suspicious stablecoin pool {:?}: price {} (expected ~1.0)",
                    pool.address, price
                );
                return false;
            }
        }
        true
    });
}
```

Call this after fetching Curve pools:

```rust
let stablecoins = get_stablecoins();
validate_stablecoin_prices(&mut ng_pool_states, &stablecoins);
```

---

### Step 5: Fix PoolState::normalized_price() for Curve Pools

**File:** `src/cartographer/fetcher.rs`

The current implementation uses the same formula for V2 and Curve. Since we're now storing accurate prices in `sqrt_price_x96`, update the Curve case:

```rust
impl PoolState {
    pub fn normalized_price(&self) -> f64 {
        match self.pool_type {
            PoolType::V3 => {
                let sp = self.sqrt_price_x96.to::<u128>() as f64;
                if sp == 0.0 { return 0.0; }
                let price_raw = (sp / 2_f64.powi(96)).powi(2);
                price_raw * 10_f64.powi(self.token0_decimals as i32 - self.token1_decimals as i32)
            }
            PoolType::Curve => {
                // For Curve pools, we now store actual get_dy price in sqrt_price_x96 format
                let sp = self.sqrt_price_x96.to::<u128>() as f64;
                if sp == 0.0 { return 0.0; }
                let price_raw = (sp / 2_f64.powi(96)).powi(2);
                // Price is already decimal-adjusted from get_dy
                price_raw
            }
            _ => {
                // V2, Balancer - use reserve ratio
                if self.liquidity == 0 || self.reserve1 == 0 { return 0.0; }
                let price = (self.reserve1 as f64 / self.liquidity as f64)
                    * 10_f64.powi(self.token0_decimals as i32 - self.token1_decimals as i32);
                if self.pool_type == PoolType::Balancer && self.weight0 != 0 {
                    let w0 = self.weight0 as f64 / 1e18;
                    return price * (w0 / (1.0 - w0));
                }
                price
            }
        }
    }
}
```

---

### Step 6: Update Simulator to Use get_dy for Curve Pools

**File:** `src/simulator/swap_simulator.rs`

Add a dedicated Curve swap simulation method:

```rust
pub async fn simulate_curve_swap(
    &self,
    pool: Address,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    fee: u32,
) -> Result<SwapResult> {
    // Determine token indices
    // For now, we'll need to query the pool for coin indices
    // This is a simplification - in production, cache this
    
    let curve_fetcher = CurveNGFetcher::new(self.rpc_url.clone());
    
    // Get i, j indices for the tokens
    // You'll need to implement this helper or look up from cached pool data
    let (i, j) = self.get_curve_token_indices(pool, token_in, token_out).await?;
    
    let dy = curve_fetcher.get_dy(pool, i, j, amount_in).await?;
    
    Ok(SwapResult {
        pool,
        token_in,
        token_out,
        amount_in,
        amount_out: dy,
        gas_used: 150_000, // Curve swaps are typically ~150k gas
        dex: Dex::Curve,
    })
}
```

Then update `simulate_cycle()` to use the correct method based on DEX type:

```rust
let result = match dex {
    Dex::UniswapV3 | Dex::SushiswapV3 | Dex::PancakeSwapV3 => {
        self.simulate_v3_swap(pool, token_in, token_out, current_amount, fee, dex).await
    }
    Dex::UniswapV2 | Dex::SushiswapV2 => {
        self.simulate_v2_swap(pool, token_in, token_out, current_amount, dex).await
    }
    Dex::Curve => {
        self.simulate_curve_swap(pool, token_in, token_out, current_amount, fee).await
    }
    Dex::BalancerV2 => {
        self.simulate_v2_swap(pool, token_in, token_out, current_amount, dex).await
    }
};
```

---

### Step 7: Add Integration Test

Create a new test file or add to existing tests:

**File:** `src/cartographer/tests/curve_pricing_test.rs` (new)

```rust
#[tokio::test]
async fn test_curve_price_accuracy() {
    // Known Curve pool: crvUSD/USDC
    let pool_address = address!("4DEcE678ceceb27446b35C672dC7d61F30bAD69E");
    let crvusd = address!("f939E0A03FB07F59A73314E73794Be0E57ac1b4E");
    let usdc = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");
    
    let fetcher = CurveNGFetcher::new(std::env::var("RPC_URL").unwrap());
    
    // Query actual price
    let dx = U256::from(10000_000000u128); // 10000 USDC (6 decimals)
    let dy = fetcher.get_dy(pool_address, 1, 0, dx).await.unwrap(); // USDC -> crvUSD
    
    // Price should be ~1.0 (within 5%)
    let price = dy.to::<u128>() as f64 / 10_f64.powi(18) / 10000.0;
    assert!(price > 0.95 && price < 1.05, "crvUSD/USDC price {} is not ~1.0", price);
    
    // Compare with balance-ratio price (should be different!)
    let pools = fetcher.discover_all_ng_pools().await.unwrap();
    let pool = pools.iter().find(|p| p.address == pool_address).unwrap();
    
    let bal0 = pool.balances[0].to::<u128>() as f64 / 10_f64.powi(18);
    let bal1 = pool.balances[1].to::<u128>() as f64 / 10_f64.powi(6);
    let balance_ratio_price = bal0 / bal1;
    
    println!("Actual get_dy price: {}", price);
    println!("Balance ratio price: {}", balance_ratio_price);
    println!("Difference: {}%", ((price - balance_ratio_price).abs() / price * 100.0));
    
    // The balance ratio might be very different due to pool imbalance
    // This test demonstrates why we need get_dy
}
```

---

## Verification Checklist

After implementing the fix, verify:

- [ ] Run the arbitrage scanner and check that stablecoin cycles show ~0.998-1.002 return (not 1.5+)
- [ ] The detailed cycle output shows reasonable prices:
  ```
  Step 1: USDC → USDe
         price=0.9985   ← Should be ~1.0 now
  ```
- [ ] All Curve pools in the graph have prices within expected ranges
- [ ] The integration test passes
- [ ] RPC call count is reasonable (batched with Multicall3)

---

## Performance Considerations

### RPC Call Overhead

The new approach adds RPC calls for `get_dy` queries. To minimize overhead:

1. **Batch all queries with Multicall3** (already supported in codebase)
2. **Cache prices for scan duration** (prices don't change much in 12 seconds)
3. **Only query high-priority pools** if needed for performance

### Estimated Additional Calls

For 50 Curve pools with 2 tokens each:
- Old: 0 price queries
- New: 50 pools × 2 directions = 100 queries → **1 Multicall3 RPC call**

This is acceptable overhead for accurate pricing.

---

## Files to Modify Summary

| File | Changes |
|------|---------|
| `src/cartographer/curve_ng.rs` | Add `batch_fetch_prices()`, modify `to_pool_state_with_price()`, deprecate old method |
| `src/cartographer/expanded_fetcher.rs` | Call new accurate price method, add validation |
| `src/cartographer/fetcher.rs` | Update `normalized_price()` for Curve pools |
| `src/simulator/swap_simulator.rs` | Add `simulate_curve_swap()` method |
| `src/cartographer/tests/` | Add integration test |

---

## Alternative Quick Fix (If Time-Constrained)

If you need a quick fix before implementing the full solution:

### Filter Curve Pools from Cycle Detection

In `src/main.rs`, filter out Curve pools before building the graph:

```rust
let pools_filtered: Vec<PoolState> = result.pool_states
    .into_iter()
    .filter(|p| p.dex != Dex::Curve)  // Exclude Curve pools entirely
    .collect();

let graph = ArbitrageGraph::from_pools(&pools_filtered);
```

**Warning:** This eliminates all Curve arbitrage opportunities but stops false positives immediately.

---

## Questions for Clarification

Before implementing, please confirm:

1. Should Curve pools be completely excluded initially, or is the full fix preferred?
2. Is there an existing cache mechanism to reuse for price caching?
3. What's the target maximum RPC calls per scan cycle?
4. Should the fix include Curve "legacy" pools (non-NG) as well?

---

## References

- Curve StableSwap invariant: `A * n^n * sum(x_i) + D = A * D * n^n + D^(n+1) / (n^n * prod(x_i))`
- `get_dy` returns actual output amount accounting for the invariant
- Balance ratios diverge from prices when pools are imbalanced (common during high volume)
