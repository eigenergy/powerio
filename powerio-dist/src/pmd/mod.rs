//! The PowerModelsDistribution ENGINEERING model as JSON ("PMD JSON").
//!
//! The byte conventions follow PMD's own `print_file`/`parse_file` pair:
//! matrices as arrays of arrays read back via `hcat` (inner arrays are
//! columns), `Inf`/`NaN` as `null` restored by field suffix (`_ub`/`max`
//! to +Inf, `_lb`/`min` to -Inf, anything else NaN), enums as uppercase
//! strings, kV and kW scales with angles in degrees, meters for lengths,
//! per unit transformer impedances, and integer terminals with grounding
//! as `grounded` plus `rg`/`xg` on the bus.

mod read;
mod write;

pub use read::{parse_pmd_file, parse_pmd_str};
pub use write::write_pmd_json;
