//! OpenDSS constructor defaults for the phase A classes.
//!
//! Values come from the object constructors in epri-dev/OpenDSS-C and are
//! verified empirically against the engine (opendssdirect via
//! `tools/verify_defaults.py`; rerun it when bumping the engine). The reader
//! materializes these into explicit model values and records each
//! materialization in `DistNetwork::defaulted`.
//!
//! The generator note: the constructor sets kW=1000, PF=0.88, kvar=60
//! (`generator.cpp`, member init), while the property display strings claim
//! kW=100, PF=0.80; the engine reports the constructor values, so those are
//! the defaults here.

pub mod line {
    //! Sequence impedances in ohm per unit length, capacitance in nF per
    //! unit length, with `units = none` (factor 1).
    pub const R1: f64 = 0.058;
    pub const X1: f64 = 0.1206;
    pub const R0: f64 = 0.1784;
    pub const X0: f64 = 0.4047;
    pub const C1_NF: f64 = 3.4;
    pub const C0_NF: f64 = 1.6;
    pub const LENGTH: f64 = 1.0;
    pub const PHASES: usize = 3;
    pub const NORMAMPS: f64 = 400.0;
}

pub mod linecode {
    pub const NPHASES: usize = 3;
}

pub mod load {
    pub const PHASES: usize = 3;
    pub const KV: f64 = 12.47;
    pub const KW: f64 = 10.0;
    pub const PF: f64 = 0.88;
    /// Constant power.
    pub const MODEL: i64 = 1;
}

pub mod transformer {
    pub const PHASES: usize = 3;
    pub const WINDINGS: usize = 2;
    pub const KV: f64 = 12.47;
    pub const KVA: f64 = 1000.0;
    pub const TAP: f64 = 1.0;
    pub const PCT_R: f64 = 0.2;
    pub const XHL: f64 = 7.0;
    pub const XHT: f64 = 35.0;
    pub const XLT: f64 = 30.0;
}

pub mod vsource {
    pub const BASEKV: f64 = 115.0;
    pub const PU: f64 = 1.0;
    pub const ANGLE_DEG: f64 = 0.0;
    pub const PHASES: usize = 3;
    pub const BUS1: &str = "sourcebus";
}

pub mod capacitor {
    pub const PHASES: usize = 3;
    pub const KVAR: f64 = 1200.0;
    pub const KV: f64 = 12.47;
}

pub mod generator {
    pub const PHASES: usize = 3;
    pub const KV: f64 = 12.47;
    pub const KW: f64 = 1000.0;
    pub const KVAR: f64 = 60.0;
}

/// Base frequency when no `Set DefaultBaseFrequency` appears.
pub const BASE_FREQUENCY: f64 = 60.0;

/// `To_Meters` from Shared/LineUnits.cpp; `none` has no factor and callers
/// treat the number as meters.
pub fn unit_to_meters(code: &str) -> Option<f64> {
    Some(match code.to_ascii_lowercase().as_str() {
        "mi" | "miles" => 1609.344,
        "kft" => 304.8,
        "km" => 1000.0,
        "m" => 1.0,
        "ft" => 0.3048,
        "in" => 0.0254,
        "cm" => 0.01,
        "mm" => 0.001,
        _ => return None,
    })
}
