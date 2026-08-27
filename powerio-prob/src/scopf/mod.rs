mod error;
mod goc3;
mod projection;

#[cfg(test)]
mod decode_tests;

pub(crate) use error::ScopfError;
pub(crate) use projection::parse_scopf_str;
