//! GridLAB-D source syntax, before preprocessing and electrical interpretation.
//!
//! This layer preserves statements and source spans. It does not execute GLM
//! scripts, resolve includes, or claim that a source is an electrical network.
//! The facade does not yet dispatch `.glm` files to a network reader.

mod syntax;

pub use syntax::{RawGlm, Statement, Token, TokenKind, parse_raw_source};
