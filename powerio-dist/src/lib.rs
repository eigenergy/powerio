//! `powerio-dist`: a multiconductor distribution network model and lossless
//! converters between OpenDSS `.dss`, PowerModelsDistribution ENGINEERING
//! JSON ("PMD JSON"), and the draft JSON schema of the IEEE PES Task Force on
//! Benchmarking Multiconductor OPF ("BMOPF JSON",
//! <https://github.com/frederikgeth/bmopf-report>).
//!
//! The canonical model is a network in wire coordinates: string bus ids,
//! ordered string terminal names per bus, explicit grounding, terminal maps
//! on every element, SI units and radians internally (BMOPF semantics, the
//! most explicit of the three formats). The transmission model in the
//! `powerio` crate is positive sequence and stays separate; the two crates
//! share conventions, not types.
//!
//! ```no_run
//! let net = powerio_dist::parse_file("feeder.dss", None)?;
//! for w in &net.warnings {
//!     eprintln!("parse: {w}");
//! }
//! let conv = net.to_format(powerio_dist::DistTargetFormat::PmdJson);
//! # Ok::<(), powerio_dist::Error>(())
//! ```
//!
//! # Fidelity contract
//!
//! The contract matches `powerio`. Writing back to the source format
//! reproduces the file byte for byte via retained source text. Every cross
//! format conversion regenerates from the typed model and reports each field
//! the target cannot represent in [`Conversion::warnings`]; nothing drops
//! silently. The dss reader materializes every OpenDSS class default into an
//! explicit model value and records which fields were defaulted
//! ([`DistNetwork::defaulted`]), so BMOPF output is always fully explicit.
//! The per fixture results live in `docs/conversion-matrix.md`.
//!
//! # Float formatting
//!
//! Canonical output formats every number as its shortest round trip
//! representation: Rust's `Display` for `.dss`, serde_json (ryu) for both
//! JSON formats. The readers parse with serde_json's `float_roundtrip`
//! feature, so a parse of canonical output recovers the exact bit pattern
//! and canonical writes are idempotent. JSON cannot carry `Inf`/`NaN`: the
//! PMD writer emits `null` (PMD restores the value from the field name
//! suffix), and the BMOPF writer emits `0` with a warning, since the schema
//! requires numbers. The byte exact echo tier is unaffected; it never
//! reformats.

pub mod bmopf;
pub mod convert;
pub mod dss;
pub mod error;
pub mod model;
pub mod pmd;

pub use bmopf::{parse_bmopf_file, parse_bmopf_str, write_bmopf_json};
pub use convert::{
    Conversion, DistTargetFormat, convert_file, convert_str, dist_target_from_name, parse_file,
    parse_str,
};
pub use dss::{parse_dss_file, parse_dss_str, write_dss};
pub use error::{Error, Result};
pub use model::{
    Configuration, DistBus, DistGenerator, DistLine, DistLineCode, DistLoad, DistNetwork,
    DistShunt, DistSourceFormat, DistSwitch, DistTransformer, Extras, UntypedObject, VoltageSource,
    Winding, WindingConn,
};
pub use pmd::{parse_pmd_file, parse_pmd_str, write_pmd_json};
