//! GridLAB-D source syntax, bounded offline preprocessing and static import.
//!
//! The source layer preserves statements and source spans without executing GLM
//! scripts or certifying electrical meaning. Raw syntax keeps
//! directives inert; preprocessing resolves only the supported local profile.
//! [`parse`] separately imports the initial phase-to-earth P/I/Z and explicit
//! line-matrix profile, rejecting unmodeled physical classes and properties.
//! The facade does not yet dispatch `.glm` files to a network reader.

mod coordinates;
mod objects;
mod preprocess;
mod project;
mod quantities;
mod read;
mod syntax;
mod units;

pub use coordinates::GlmCoordinates;
pub use objects::{GlmObject, GlmProperty, GlmPropertyValue, ObjectIndex, ObjectKey};
pub use preprocess::{
    ExpansionOrigin, GlmDialect, PreparedGlm, PreprocessOptions, preprocess_source,
};
pub use project::{GlmProject, parse_project};
pub use quantities::{GlmQuantity, GlmQuantityKind};
pub use read::parse;
pub use syntax::{RawGlm, Statement, Token, TokenKind, parse_raw_source};
