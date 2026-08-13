mod error;
mod goc3;
pub mod json;
mod projection;
mod types;

pub use error::{ScopfError, ScopfResult};
pub use projection::build_scopf_instance_from_str;
pub use types::*;
