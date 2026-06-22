//! Tick pricing engine — canonical Rust implementation of the multiplier math.
//!
//! Spec: `games/tap-trading/docs/MATH_SPEC.md`.

pub mod bgk;
pub mod constants;
pub mod error;
pub mod hui;
pub mod multiplier;
pub mod types;
pub mod vol;

pub use bgk::apply_bgk_correction;
pub use constants::{BETA_BGK, EPSILON, SECONDS_PER_YEAR};
pub use error::PricingError;
pub use hui::hui_no_touch;
pub use multiplier::{compute_multiplier, compute_p_touch, first_passage_touch_prob};
pub use types::{AssetSymbol, Cell, OracleState, PricingConfig};
pub use vol::{estimate_realized_vol, jump_adjusted_sigma};
