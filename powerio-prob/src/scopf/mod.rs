mod error;
mod goc3;
pub mod json;
mod projection;
mod types;

pub use error::{ScopfError, ScopfResult};
pub use projection::parse_scopf_str;
pub use types::*;
