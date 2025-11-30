# High-Volatility Q4 2025 Tokens Implementation Guide

## For Claude Code Opus - Zero Executor Modifications

> **CRITICAL CONSTRAINT**: The executor contract (`ArbitrageExecutor.sol`) is already deployed and signed on-chain. **DO NOT MODIFY** any files in `src/executor/`. All changes must be data-layer only.

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Safe vs Forbidden Files](#safe-vs-forbidden-files)
3. [Implementation Tasks](#implementation-tasks)
   - [Task 1: Add New Token Categories](#task-1-add-new-token-categories)
   - [Task 2: Add AI/Compute Tokens](#task-2-add-aicompute-tokens)
   - [Task 3: Add Gaming Tokens](#task-3-add-gaming-tokens)
   - [Task 4: Add Meme Tokens](#task-4-add-meme-tokens)
   - [Task 5: Add Restaking Tokens](#task-5-add-restaking-tokens)
   - [Task 6: Add RWA Tokens](#task-6-add-rwa-tokens)
   - [Task 7: Add Pool Definitions](#task-7-add-pool-definitions)
   - [Task 8: Update Symbol Maps](#task-8-update-symbol-maps)
   - [Task 9: Add Decimals Support](#task-9-add-decimals-support)
   - [Task 10: Update Base Tokens](#task-10-update-base-tokens)
4. [Verification Steps](#verification-steps)
5. [Testing](#testing)

---

## Architecture Overview

The Sniper bot has a clear separation of concerns:

```
┌─────────────────────────────────────────────────────────────────┐
│  DATA LAYER (SAFE TO MODIFY)                                    │
│  ─────────────────────────────────────────────────────────────  │
│  src/tokens.rs          → Token definitions & categories        │
│  src/cartographer/      → Pool discovery & graph construction   │
│    fetcher.rs           → Static pool definitions               │
│    expanded_fetcher.rs  → Extended pools & symbol maps          │
│    graph.rs             → Arbitrage graph (auto-uses new pools) │
│  src/config.rs          → Configuration defaults                │
└─────────────────────────────────────────────────────────────────┘
                              ↓
                     Automatic Integration
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│  EXECUTION LAYER (DO NOT MODIFY)                                │
│  ─────────────────────────────────────────────────────────────  │
│  src/executor/          → Deployed contract interaction         │
│    flash_loan.rs        → DexType enum matches on-chain         │
│    mod.rs               → Execution engine                      │
│    signer.rs            → Transaction signing                   │
│    flashbots.rs         → Bundle submission                     │
└─────────────────────────────────────────────────────────────────┘
```

**Key Insight**: New tokens and pools automatically flow through the graph construction and cycle detection. The executor only cares about DEX type (V2, V3, Curve, etc.) - not specific tokens.

---

## Safe vs Forbidden Files

### ✅ SAFE TO MODIFY

| File | Purpose | What to Change |
|------|---------|----------------|
| `src/tokens.rs` | Token definitions | Add new tokens, categories |
| `src/cartographer/fetcher.rs` | Pool definitions | Add new `PoolInfo` entries |
| `src/cartographer/expanded_fetcher.rs` | Extended tokens/pools | Add to `get_priority_tokens()`, symbol maps |
| `src/config.rs` | Defaults | Update `default_base_tokens()` if needed |

### ⛔ DO NOT MODIFY

| File | Reason |
|------|--------|
| `src/executor/flash_loan.rs` | Contains `DexType` enum matching deployed contract |
| `src/executor/mod.rs` | Production execution logic |
| `src/executor/signer.rs` | Signing logic for deployed contract |
| `src/executor/flashbots.rs` | Bundle submission logic |
| `ArbitrageExecutor.sol` | Already deployed on-chain |

### Supported DEX Types (Immutable - Already in Contract)

```rust
// From src/executor/flash_loan.rs - DO NOT CHANGE
pub enum DexType {
    UniswapV3 = 0,    // Also used for SushiswapV3
    UniswapV2 = 1,
    SushiswapV2 = 2,
    PancakeSwapV3 = 3,
    BalancerV2 = 4,
    Curve = 5,
}
```

All new pools MUST use one of these DEX types.

---

## Implementation Tasks

### Task 1: Add New Token Categories

**File**: `src/tokens.rs`

**Location**: Find the `TokenCategory` enum (around line 30-50)

**Add these categories**:

```rust
/// Token categories for filtering and analysis
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenCategory {
    // ... existing categories ...
    
    /// AI/Compute tokens (RNDR, FET, AGIX, TAO)
    AICompute,
    
    /// Gaming/Metaverse tokens (IMX, GALA, SAND, AXS)
    Gaming,
    
    /// Restaking tokens (EIGEN, pufETH, ezETH, weETH)
    Restaking,
    
    /// Real World Asset tokens (ONDO, USDY, OUSG)
    RWA,
}
```

---

### Task 2: Add AI/Compute Tokens

**File**: `src/tokens.rs`

**Add new function after `meme_tokens()`**:

```rust
// ============================================
// AI/COMPUTE TOKENS (High Volatility - Catalyst Driven)
// ============================================

pub fn ai_compute_tokens() -> Vec<Token> {
    vec![
        Token {
            symbol: "RNDR",
            address: Address::from_str("0x6de037ef9ad2725eb40118bb1702ebb27e4aeb24").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::AICompute,
        },
        Token {
            symbol: "FET",
            address: Address::from_str("0xaea46A60368A7bD060eec7DF8CBa43b7EF41Ad85").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::AICompute,
        },
        Token {
            symbol: "AGIX",
            address: Address::from_str("0x5B7533812759B45C2B44C19e320ba2cD2681b542").unwrap(),
            decimals: 8,
            is_base: false,
            category: TokenCategory::AICompute,
        },
        Token {
            symbol: "wTAO",
            address: Address::from_str("0x77e06c9eccf2e797fd462a92b6d7642ef85b0a44").unwrap(),
            decimals: 9,
            is_base: false,
            category: TokenCategory::AICompute,
        },
        // Staked TAO - yield-drift arbitrage opportunity vs wTAO
        Token {
            symbol: "stTAO",
            address: Address::from_str("0xb60acd2057067dc9ed8c083f5aa227a244044fd6").unwrap(),
            decimals: 9,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
    ]
}
```

---

### Task 3: Add Gaming Tokens

**File**: `src/tokens.rs`

**Add new function**:

```rust
// ============================================
// GAMING/METAVERSE TOKENS
// ============================================

pub fn gaming_tokens() -> Vec<Token> {
    vec![
        Token {
            symbol: "IMX",
            address: Address::from_str("0xf57e7e7c23978c3caec3c3548e3d615c346e79ff").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Gaming,
        },
        Token {
            symbol: "GALA",
            address: Address::from_str("0xd1d2eb1b1e90b638588728b4130137d262c87cae").unwrap(),
            decimals: 8,
            is_base: false,
            category: TokenCategory::Gaming,
        },
        Token {
            symbol: "SAND",
            address: Address::from_str("0x3845badAde8e6dFF049820680d1F14bD3903a5d0").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Gaming,
        },
        Token {
            symbol: "AXS",
            address: Address::from_str("0xbb0e17ef65f82ab018d8edd776e8dd940327b28b").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Gaming,
        },
    ]
}
```

---

### Task 4: Add Meme Tokens

**File**: `src/tokens.rs`

**Update existing `meme_tokens()` function to add**:

```rust
// Add to existing meme_tokens() function:
Token {
    symbol: "MOG",
    address: Address::from_str("0xaaee1a9723aadb7afa2810263653a34ba2c21c7a").unwrap(),
    decimals: 18,
    is_base: false,
    category: TokenCategory::Meme,
},
Token {
    symbol: "SPX6900",
    address: Address::from_str("0xe0f63a424a4439cbe457d80e4f4b51ad25b2c56c").unwrap(),
    decimals: 8,
    is_base: false,
    category: TokenCategory::Meme,
},
Token {
    symbol: "TURBO",
    address: Address::from_str("0xa35923162c49cf95e6bf26623385eb431ad920d3").unwrap(),
    decimals: 18,
    is_base: false,
    category: TokenCategory::Meme,
},
Token {
    symbol: "FLOKI",
    address: Address::from_str("0xcf0c122c6b73ff809c693db761e7baebe62b6a2e").unwrap(),
    decimals: 9,
    is_base: false,
    category: TokenCategory::Meme,
},
```

---

### Task 5: Add Restaking Tokens

**File**: `src/tokens.rs`

**Add new function**:

```rust
// ============================================
// RESTAKING TOKENS (NAV Discount Arbitrage)
// ============================================

pub fn restaking_tokens() -> Vec<Token> {
    vec![
        // Governance tokens
        Token {
            symbol: "EIGEN",
            address: Address::from_str("0xec53bF9167f50cDEB3Ae105f56099aaaB9061F83").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Restaking,
        },
        Token {
            symbol: "REZ",
            address: Address::from_str("0x3B50805453023a91a8bf641e279401a0b23FA6F9").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Restaking,
        },
        Token {
            symbol: "PUFFER",
            address: Address::from_str("0x4d1C297d39C5c1277964D0E3f8Aa901493664530").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Restaking,
        },
        
        // Liquid Restaking Tokens (LRTs) - yield-bearing
        Token {
            symbol: "pufETH",
            address: Address::from_str("0xD9A442856C234a39a81a089C06451EBAa4306a72").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
        Token {
            symbol: "ezETH",
            address: Address::from_str("0xbf5495Efe5DB9ce00f80364C8B423567e58d2110").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
        Token {
            symbol: "weETH",
            address: Address::from_str("0xCd5fE23C85820F7B72D0926FC9b05b43E359b7ee").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
        Token {
            symbol: "eETH",
            address: Address::from_str("0x35fA164735182de50811E8e2E824cFb9B6118ac2").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::LiquidStaking,
        },
    ]
}
```

---

### Task 6: Add RWA Tokens

**File**: `src/tokens.rs`

**Add new function**:

```rust
// ============================================
// RWA (Real World Asset) TOKENS
// ============================================

pub fn rwa_tokens() -> Vec<Token> {
    vec![
        // Governance
        Token {
            symbol: "ONDO",
            address: Address::from_str("0xfAbA6f8e4a5E8Ab82F62fe7C39859FA577269BE3").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::RWA,
        },
        Token {
            symbol: "CFG",
            address: Address::from_str("0xc221b7e65ffc80de234bbb6667abdd46593d34f0").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::RWA,
        },
        Token {
            symbol: "SYRUP",
            address: Address::from_str("0x643C4E15d7d62Ad0aBeC4a9BD4b001aA3Ef52d66").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::RWA,
        },
        
        // Yield-bearing RWA tokens
        Token {
            symbol: "USDY",
            address: Address::from_str("0x96F6eF951840721AdBF46Ac996b59E0235CB985C").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
        Token {
            symbol: "OUSG",
            address: Address::from_str("0x1B19C19393e2d034D8Ff31ff34c81252FcBbee92").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
        Token {
            symbol: "rOUSG",
            address: Address::from_str("0xaf37c1167910ebC994e266949387d2c7C326b879").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
    ]
}
```

---

### Task 7: Add Pool Definitions

**File**: `src/cartographer/fetcher.rs`

**Location**: Find the `get_all_known_pools()` function

**Add these pools to the existing vector**:

```rust
// ============================================
// AI/COMPUTE TOKEN POOLS
// ============================================

// RNDR - Multi-tier for fee arbitrage
PoolInfo { address: "0xe936f0073549ad8b1fa53583600d629ba9375161", token0_symbol: "RNDR", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
PoolInfo { address: "0x4628a0a564debfc8798eb55db5c91f2200486c24", token0_symbol: "RNDR", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },

// FET - Multi-tier
PoolInfo { address: "0x948b54a93f5ad1df6b8bff6dc249d99ca2eca052", token0_symbol: "FET", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
PoolInfo { address: "0x744159757cac173a7a3ecf5e97adb10d1a725377", token0_symbol: "FET", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },

// wTAO
PoolInfo { address: "0x2982d3295a0e1a99e6e88ece0e93ffdfc5c761ae", token0_symbol: "wTAO", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
PoolInfo { address: "0xf763bb342eb3d23c02ccb86312422fe0c1c17e94", token0_symbol: "wTAO", token1_symbol: "USDC", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },

// stTAO (yield-bearing) - for wTAO/stTAO arbitrage
PoolInfo { address: "0xb60acd2057067dc9ed8c083f5aa227a244044fd6", token0_symbol: "stTAO", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },

// ============================================
// GAMING TOKEN POOLS
// ============================================

// IMX
PoolInfo { address: "0xFd76bE67FFF3BAC84E3D5444167bbc018f5968b6", token0_symbol: "IMX", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },

// GALA
PoolInfo { address: "0x465e56cd21ad47d4d4790f17de5e0458f20c3719", token0_symbol: "GALA", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },

// SAND - V2 pool (higher liquidity)
PoolInfo { address: "0x3dd49f67e9d5bc4c5e6634b3f70bfd9dc1b6bd74", token0_symbol: "SAND", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },

// AXS
PoolInfo { address: "0x3019d4e366576a88d28b623afaf3ecb9ec9d9580", token0_symbol: "AXS", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },

// ============================================
// MEME TOKEN POOLS (V2/V3 DUAL - PRIMARY ARB TARGETS)
// ============================================

// MOG - V2 ($12M liquidity) and V3 ($229K) - ideal fee-tier arbitrage
PoolInfo { address: "0xc2eab7d33d3cb97692ecb231a5d0e4a649cb539d", token0_symbol: "MOG", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
PoolInfo { address: "0x7832310cd0de39c4ce0a635f34d9a4b5b47fd434", token0_symbol: "MOG", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },

// SPX6900 - V2 only ($13M)
PoolInfo { address: "0x52c77b0cb827afbad022e6d6caf2c44452edbc39", token0_symbol: "SPX6900", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },

// TURBO
PoolInfo { address: "0x7baece5d47f1bc5e1953fbe0e9931d54dab6d810", token0_symbol: "TURBO", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },

// FLOKI
PoolInfo { address: "0x7929d24b5bc6e06bfc7a0d5e51c340c2ad952f69", token0_symbol: "FLOKI", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },

// ============================================
// RESTAKING TOKEN POOLS
// ============================================

// EIGEN
PoolInfo { address: "0xc2c390c6cd3c4e6c2b70727d35a45e8a072f18ca", token0_symbol: "EIGEN", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },

// ezETH - Balancer stable pool
PoolInfo { address: "0x596192bb6e41802428ac943d2f1476c1af25cc0e", token0_symbol: "ezETH", token1_symbol: "WETH", fee: 50, dex: Dex::BalancerV2, pool_type: PoolType::Balancer, weight0: Some(0.5) },

// weETH - Curve pool
PoolInfo { address: "0x13947303f63b363876868d070f14dc865c36463b", token0_symbol: "weETH", token1_symbol: "WETH", fee: 4, dex: Dex::Curve, pool_type: PoolType::Curve, weight0: None },

// pufETH - Curve NG pool (NAV discount arbitrage target)
PoolInfo { address: "0xB3c8Ce1eE157b0DCAa96897C9170aEe6281706c9", token0_symbol: "pufETH", token1_symbol: "wstETH", fee: 4, dex: Dex::Curve, pool_type: PoolType::Curve, weight0: None },

// ============================================
// RWA TOKEN POOLS
// ============================================

// ONDO
PoolInfo { address: "0x7b1e5d984a43ee732de195628d20d05cfabc3cc7", token0_symbol: "ONDO", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },

// SYRUP
PoolInfo { address: "0x11e451c1f5cb0c0d2885c3e8687b14bcf9b0c82d", token0_symbol: "SYRUP", token1_symbol: "USDC", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
```

---

### Task 8: Update Symbol Maps

**File**: `src/cartographer/expanded_fetcher.rs`

**Location**: Find `build_expanded_symbol_map()` function

**Add to the `additional` array**:

```rust
let additional: [(&str, &str); 35] = [
    // ... existing entries ...
    
    // AI/Compute
    ("0x6de037ef9ad2725eb40118bb1702ebb27e4aeb24", "RNDR"),
    ("0xaea46A60368A7bD060eec7DF8CBa43b7EF41Ad85", "FET"),
    ("0x5B7533812759B45C2B44C19e320ba2cD2681b542", "AGIX"),
    ("0x77e06c9eccf2e797fd462a92b6d7642ef85b0a44", "wTAO"),
    ("0xb60acd2057067dc9ed8c083f5aa227a244044fd6", "stTAO"),
    
    // Gaming
    ("0xf57e7e7c23978c3caec3c3548e3d615c346e79ff", "IMX"),
    ("0xd1d2eb1b1e90b638588728b4130137d262c87cae", "GALA"),
    ("0x3845badAde8e6dFF049820680d1F14bD3903a5d0", "SAND"),
    ("0xbb0e17ef65f82ab018d8edd776e8dd940327b28b", "AXS"),
    
    // Meme
    ("0xaaee1a9723aadb7afa2810263653a34ba2c21c7a", "MOG"),
    ("0xe0f63a424a4439cbe457d80e4f4b51ad25b2c56c", "SPX6900"),
    ("0xa35923162c49cf95e6bf26623385eb431ad920d3", "TURBO"),
    ("0xcf0c122c6b73ff809c693db761e7baebe62b6a2e", "FLOKI"),
    
    // Restaking
    ("0xec53bF9167f50cDEB3Ae105f56099aaaB9061F83", "EIGEN"),
    ("0x3B50805453023a91a8bf641e279401a0b23FA6F9", "REZ"),
    ("0x4d1C297d39C5c1277964D0E3f8Aa901493664530", "PUFFER"),
    ("0xD9A442856C234a39a81a089C06451EBAa4306a72", "pufETH"),
    ("0xbf5495Efe5DB9ce00f80364C8B423567e58d2110", "ezETH"),
    ("0x35fA164735182de50811E8e2E824cFb9B6118ac2", "eETH"),
    
    // RWA
    ("0xfAbA6f8e4a5E8Ab82F62fe7C39859FA577269BE3", "ONDO"),
    ("0xc221b7e65ffc80de234bbb6667abdd46593d34f0", "CFG"),
    ("0x643C4E15d7d62Ad0aBeC4a9BD4b001aA3Ef52d66", "SYRUP"),
    ("0x96F6eF951840721AdBF46Ac996b59E0235CB985C", "USDY"),
    ("0x1B19C19393e2d034D8Ff31ff34c81252FcBbee92", "OUSG"),
    ("0xaf37c1167910ebC994e266949387d2c7C326b879", "rOUSG"),
];
```

**Also update `get_priority_tokens()`** to include high-priority volatile tokens:

```rust
pub fn get_priority_tokens() -> Vec<(Address, &'static str, u8)> {
    vec![
        // ... existing base tokens ...
        
        // HIGH VOLATILITY ADDITIONS
        // AI tokens
        (address!("6de037ef9ad2725eb40118bb1702ebb27e4aeb24"), "RNDR", 18),
        (address!("aea46A60368A7bD060eec7DF8CBa43b7EF41Ad85"), "FET", 18),
        (address!("77e06c9eccf2e797fd462a92b6d7642ef85b0a44"), "wTAO", 9),
        
        // Meme (V2/V3 arb targets)
        (address!("aaee1a9723aadb7afa2810263653a34ba2c21c7a"), "MOG", 18),
        (address!("e0f63a424a4439cbe457d80e4f4b51ad25b2c56c"), "SPX6900", 8),
        
        // Restaking (NAV discount)
        (address!("ec53bF9167f50cDEB3Ae105f56099aaaB9061F83"), "EIGEN", 18),
        (address!("D9A442856C234a39a81a089C06451EBAa4306a72"), "pufETH", 18),
        (address!("bf5495Efe5DB9ce00f80364C8B423567e58d2110"), "ezETH", 18),
        
        // RWA
        (address!("fAbA6f8e4a5E8Ab82F62fe7C39859FA577269BE3"), "ONDO", 18),
        (address!("96F6eF951840721AdBF46Ac996b59E0235CB985C"), "USDY", 18),
    ]
}
```

---

### Task 9: Add Decimals Support

**File**: `src/cartographer/fetcher.rs`

**Location**: Find `get_token_decimals()` function

**Update to handle non-standard decimals**:

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
    
    // 8 decimals
    if a.contains("2260fac5e5542a773aa44fbcfedf7c193bc2c599")  // WBTC
        || a.contains("5b7533812759b45c2b44c19e320ba2cd2681b542")  // AGIX
        || a.contains("d1d2eb1b1e90b638588728b4130137d262c87cae")  // GALA
        || a.contains("e0f63a424a4439cbe457d80e4f4b51ad25b2c56c")  // SPX6900
    {
        return 8;
    }
    
    // 9 decimals (TAO ecosystem, FLOKI)
    if a.contains("77e06c9eccf2e797fd462a92b6d7642ef85b0a44")  // wTAO
        || a.contains("b60acd2057067dc9ed8c083f5aa227a244044fd6")  // stTAO
        || a.contains("cf0c122c6b73ff809c693db761e7baebe62b6a2e")  // FLOKI
    {
        return 9;
    }
    
    // Default: 18 decimals
    18
}
```

---

### Task 10: Update Base Tokens

**File**: `src/tokens.rs`

**Location**: Find `all_tokens()` function

**Update to include new token categories**:

```rust
/// Get all tokens (base + all categories)
pub fn all_tokens() -> Vec<Token> {
    let mut tokens = base_tokens();
    tokens.extend(sky_ecosystem_tokens());
    tokens.extend(usd3_ecosystem_tokens());
    tokens.extend(algo_stable_tokens());
    tokens.extend(lsd_tokens());
    tokens.extend(defi_tokens());
    tokens.extend(meme_tokens());
    // NEW ADDITIONS
    tokens.extend(ai_compute_tokens());
    tokens.extend(gaming_tokens());
    tokens.extend(restaking_tokens());
    tokens.extend(rwa_tokens());
    tokens
}
```

**Add helper functions for filtering**:

```rust
/// Get all AI/Compute tokens
pub fn all_ai_tokens() -> Vec<Token> {
    all_tokens().into_iter()
        .filter(|t| t.category == TokenCategory::AICompute)
        .collect()
}

/// Get all gaming tokens
pub fn all_gaming_tokens() -> Vec<Token> {
    all_tokens().into_iter()
        .filter(|t| t.category == TokenCategory::Gaming)
        .collect()
}

/// Get all restaking tokens (including LRTs)
pub fn all_restaking_tokens() -> Vec<Token> {
    all_tokens().into_iter()
        .filter(|t| t.category == TokenCategory::Restaking)
        .collect()
}

/// Get all RWA tokens
pub fn all_rwa_tokens() -> Vec<Token> {
    all_tokens().into_iter()
        .filter(|t| t.category == TokenCategory::RWA)
        .collect()
}
```

---

## Verification Steps

After implementing all tasks, verify the changes:

### 1. Compile Check

```bash
cargo build --release
```

Expected: No errors. All new tokens and pools should compile cleanly.

### 2. Token Count Verification

```bash
cargo test test_all_tokens_populated -- --nocapture
```

Update the test assertion to expect the new token count:

```rust
#[test]
fn test_all_tokens_populated() {
    let tokens = all_tokens();
    assert!(tokens.len() >= 50, "Should have at least 50 tokens after expansion");
}
```

### 3. Symbol Map Verification

Add a new test in `src/tokens.rs`:

```rust
#[test]
fn test_new_tokens_in_symbol_map() {
    let symbols = build_symbol_map();
    
    // AI tokens
    let rndr = Address::from_str("0x6de037ef9ad2725eb40118bb1702ebb27e4aeb24").unwrap();
    assert_eq!(symbols.get(&rndr), Some(&"RNDR"));
    
    // Meme tokens
    let mog = Address::from_str("0xaaee1a9723aadb7afa2810263653a34ba2c21c7a").unwrap();
    assert_eq!(symbols.get(&mog), Some(&"MOG"));
    
    // Restaking
    let eigen = Address::from_str("0xec53bF9167f50cDEB3Ae105f56099aaaB9061F83").unwrap();
    assert_eq!(symbols.get(&eigen), Some(&"EIGEN"));
}
```

### 4. Pool Discovery Test

```bash
cargo run --bin discover-pools
```

The new pools should appear in the discovery output.

### 5. Full Integration Test

```bash
# Run in simulation mode
EXECUTION_MODE=simulation cargo run --release
```

Watch for:
- New tokens appearing in the graph
- Cycles being found through new pools
- No executor errors (confirms no unintended changes)

---

## Testing

### Unit Tests to Add

**File**: `src/tokens.rs` (append to existing tests)

```rust
#[test]
fn test_ai_compute_tokens() {
    let tokens = ai_compute_tokens();
    assert!(tokens.len() >= 5, "Should have RNDR, FET, AGIX, wTAO, stTAO");
    
    // Verify decimals
    let wtao = tokens.iter().find(|t| t.symbol == "wTAO").unwrap();
    assert_eq!(wtao.decimals, 9);
}

#[test]
fn test_gaming_tokens() {
    let tokens = gaming_tokens();
    assert!(tokens.len() >= 4, "Should have IMX, GALA, SAND, AXS");
}

#[test]
fn test_restaking_tokens() {
    let tokens = restaking_tokens();
    assert!(tokens.len() >= 7, "Should have EIGEN, REZ, PUFFER, pufETH, ezETH, weETH, eETH");
    
    // pufETH should be yield-bearing
    let pufeth = tokens.iter().find(|t| t.symbol == "pufETH").unwrap();
    assert_eq!(pufeth.category, TokenCategory::YieldBearing);
}

#[test]
fn test_rwa_tokens() {
    let tokens = rwa_tokens();
    assert!(tokens.len() >= 6, "Should have ONDO, CFG, SYRUP, USDY, OUSG, rOUSG");
    
    // USDY should be yield-bearing
    let usdy = tokens.iter().find(|t| t.symbol == "USDY").unwrap();
    assert_eq!(usdy.category, TokenCategory::YieldBearing);
}

#[test]
fn test_decimals_for_new_tokens() {
    // AGIX is 8 decimals
    let agix = Address::from_str("0x5B7533812759B45C2B44C19e320ba2cD2681b542").unwrap();
    assert_eq!(get_token_decimals(&agix), 8);
    
    // wTAO is 9 decimals
    let wtao = Address::from_str("0x77e06c9eccf2e797fd462a92b6d7642ef85b0a44").unwrap();
    assert_eq!(get_token_decimals(&wtao), 9);
}
```

### Run All Tests

```bash
cargo test
```

---

## Summary Checklist

Before marking complete, verify:

- [ ] `TokenCategory` enum has `AICompute`, `Gaming`, `Restaking`, `RWA`
- [ ] `ai_compute_tokens()` function exists with 5 tokens
- [ ] `gaming_tokens()` function exists with 4 tokens
- [ ] `restaking_tokens()` function exists with 7 tokens
- [ ] `rwa_tokens()` function exists with 6 tokens
- [ ] `meme_tokens()` updated with MOG, SPX6900, TURBO, FLOKI
- [ ] `all_tokens()` includes all new token functions
- [ ] `get_all_known_pools()` has ~20 new pool entries
- [ ] `build_expanded_symbol_map()` has all new token symbols
- [ ] `get_priority_tokens()` includes high-priority volatile tokens
- [ ] `get_token_decimals()` handles 6, 8, 9 decimal tokens
- [ ] All tests pass
- [ ] `cargo build --release` succeeds
- [ ] **NO changes to `src/executor/` directory**

---

## Priority Arbitrage Opportunities (Reference)

The implemented tokens enable these specific arbitrage strategies:

1. **MOG V2↔V3 Fee-Tier Arbitrage**: $12M V2 vs $229K V3, 0.5-2% spreads
2. **pufETH NAV Discount**: Persistent discount on Curve, redeem via PufferVault
3. **PEPE V2↔V3 Spread**: $32M V2 liquidity
4. **wTAO/stTAO Yield Drift**: stTAO accrues staking rewards
5. **ezETH Cross-Pool**: Balancer vs Curve vs Uniswap price discrepancies
6. **FET Multi-Tier**: 1% vs 0.3% pool arbitrage
7. **USDY/USDC Drift**: ~5% annual drift creates NAV arbitrage
