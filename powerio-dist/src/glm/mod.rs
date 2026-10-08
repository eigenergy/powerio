//! GridLAB-D source syntax and bounded offline preprocessing.
//!
//! This layer preserves statements and source spans. It does not execute GLM
//! scripts or claim that a source is an electrical network. Raw syntax keeps
//! directives inert; preprocessing resolves only the supported local profile.
//! The facade does not yet dispatch `.glm` files to a network reader.

mod coordinates;
mod objects;
mod preprocess;
mod project;
mod syntax;

pub use coordinates::GlmCoordinates;
pub use objects::{GlmObject, GlmProperty, GlmPropertyValue, ObjectIndex, ObjectKey};
pub use preprocess::{
    ExpansionOrigin, GlmDialect, PreparedGlm, PreprocessOptions, preprocess_source,
};
pub use project::{GlmProject, parse_project};
pub use syntax::{RawGlm, Statement, Token, TokenKind, parse_raw_source};
