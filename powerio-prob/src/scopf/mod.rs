mod error;
mod goc3;
mod projection;
mod types;
pub mod wire;

pub use error::{ScopfError, ScopfResult};
pub use projection::build_scopf_instance_from_str;
pub use types::*;
