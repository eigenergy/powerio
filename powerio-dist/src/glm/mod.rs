//! GridLAB-D source syntax, bounded offline preprocessing and static import.
//!
//! The source layer preserves statements and source spans without executing GLM
//! scripts or certifying electrical meaning. Raw syntax keeps
//! directives inert; preprocessing resolves only the supported local profile.
//! [`parse`] separately imports phase-to-earth and explicit phase-pair P/I/Z
//! loads and the explicit line-matrix profile, rejecting unmodeled physics.
//! Distribution and facade dispatch select PNNL 5.3 for `.glm`/`gridlabd`, or
//! Arras 4.3 for `gridlabd-arras`. Same-profile emission replays acquired project
//! files; [`emit_canonical`] generates a single file from current typed values.

mod connections;
mod coordinates;
mod emit_project;
mod objects;
mod preprocess;
mod project;
mod quantities;
mod read;
mod syntax;
mod units;
mod write;

pub use coordinates::GlmCoordinates;
pub(crate) use emit_project::retained_project;
pub use objects::{GlmObject, GlmProperty, GlmPropertyValue, ObjectIndex, ObjectKey};
pub use preprocess::{
    ExpansionOrigin, GlmDialect, PreparedGlm, PreprocessOptions, preprocess_source,
};
pub use project::{GlmProject, parse_project};
pub use quantities::{GlmQuantity, GlmQuantityKind};
pub use read::parse;
pub use syntax::{RawGlm, Statement, Token, TokenKind, parse_raw_source};
pub use write::emit_canonical;
pub(crate) use write::emit_text as emit_glm_text;
