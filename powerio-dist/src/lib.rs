//! `powerio-dist`: a multiconductor distribution network model and lossless
//! converters between OpenDSS `.dss`, PowerModelsDistribution ENGINEERING
//! JSON, and the draft BMOPF task force JSON schema.
//!
//! The canonical model is a network in wire coordinates: string bus ids,
//! ordered string terminal names per bus, explicit grounding, terminal maps
//! on every element, SI units and radians internally. The transmission model
//! in the `powerio` crate is positive sequence and stays separate; the two
//! crates share conventions, not types.
//!
//! The fidelity contract matches `powerio`: writing back to the source format
//! reproduces the file byte for byte via retained source text, and every
//! cross-format conversion reports each field the target cannot represent.
//! Nothing drops silently.

pub mod error;

pub use error::{Error, Result};
