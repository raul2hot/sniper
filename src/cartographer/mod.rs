//! Phase 1: The Cartographer (Data Ingest) - EXPANDED Edition
//!
//! Now includes:
//! - Existing Uniswap V2/V3, SushiSwap, PancakeSwap pools
//! - NEW: Curve StableSwap NG (dynamic discovery + dynamic fees)
//! - NEW: Sky Ecosystem (sUSDS, USDS - ERC-4626 yield arbitrage)
//! - NEW: USD3/Reserve Protocol (NAV arbitrage)
//!
//! Multicall3 for efficient batch fetching!

mod fetcher;
mod graph;

// NEW MODULES - Phase 1-4
pub mod curve_ng;
pub mod sky_ecosystem;
pub mod usd3_reserve;
pub mod expanded_fetcher;

// Re-exports from original fetcher
pub use fetcher::{PoolFetcher, PoolState, Dex, PoolType, get_token_decimals, get_all_known_pools, PoolInfo};
pub use graph::{ArbitrageGraph, EdgeData};

// Re-exports from new modules
pub use curve_ng::{
    CurveNGFetcher,
    CurveNGPool,
    CurveNGFactoryType,
    CURVE_NG_FACTORY,
    CURVE_TWOCRYPTO_NG_FACTORY,
    CURVE_TRICRYPTO_NG_FACTORY,
    get_priority_curve_ng_pools,
};

pub use sky_ecosystem::{
    SkyAdapter,
    ERC4626State,
    VirtualERC4626Pool,
    ERC4626Direction,
    YieldDriftArb,
    ArbDirection,
    USDS_TOKEN,
    SUSDS_TOKEN,
    SKY_TOKEN,
    DAI_TOKEN as SKY_DAI_TOKEN,
    SDAI_TOKEN,
    DAI_USDS_CONVERTER,
    is_sky_ecosystem_token,
    get_sky_token_symbol,
    get_all_erc4626_vaults,
    create_erc4626_virtual_pools,
};

pub use usd3_reserve::{
    USD3Adapter,
    USD3State,
    BasketComponent,
    NAVArbitrage,
    NAVArbDirection,
    USD3_TOKEN,
    PYUSD_TOKEN,
    CUSDC_TOKEN,
    get_known_rtokens,
    get_known_yield_tokens,
    get_usd3_curve_pools,
    is_usd3_ecosystem_token,
};

pub use expanded_fetcher::{
    ExpandedPoolFetcher,
    ExpandedPoolResult,
    SpecialOpportunity,
    get_priority_tokens,
    build_expanded_symbol_map,
    get_new_priority_pools,
    NewPoolInfo,
    check_special_opportunities,
};