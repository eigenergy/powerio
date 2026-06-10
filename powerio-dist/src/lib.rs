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

pub mod bmopf;
pub mod convert;
pub mod dss;
pub mod error;
pub mod model;

pub use bmopf::{parse_bmopf_file, parse_bmopf_str, write_bmopf_json};
pub use convert::Conversion;
pub use dss::{parse_dss_file, parse_dss_str};
pub use error::{Error, Result};
pub use model::{
    Configuration, DistBus, DistGenerator, DistLine, DistLineCode, DistLoad, DistNetwork,
    DistShunt, DistSourceFormat, DistSwitch, DistTransformer, Extras, UntypedObject, VoltageSource,
    Winding, WindingConn,
};
