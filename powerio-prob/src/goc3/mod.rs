mod decode;
mod error;
mod projection;

#[cfg(test)]
mod decode_tests;

pub(crate) use error::Goc3Error;
pub(crate) use projection::parse_goc3_document;
