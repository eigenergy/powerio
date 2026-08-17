//! The diagnostic vocabulary shared by every PowerIO crate.
//!
//! A free-form `Vec<String>` warning is useful for a human and opaque to CI, an
//! agent, or a downstream solver. Every finding a reader, a lowering pass, or a
//! writer records carries a stable [`DiagnosticCode`], a [`DiagnosticSeverity`],
//! a human message, and where known an element path, a [`SourceRef`], a details
//! object, and a suggested action. The structured record is primary and the
//! text lines are rendered from it by [`render_line`], so the two cannot
//! disagree.
//!
//! A code reads `NAMESPACE.SCOPE.SPECIFIC`. The first segment names the stage
//! ([`DiagnosticStage`]) and is the only segment a consumer parses; the rest is
//! opaque identity. powerio never emits a code whose first segment is outside
//! the ten, so a downstream producer picks any other first segment and its
//! codes merge into one report without coordination.
//!
//! The crate is a leaf on purpose: the transmission model, the distribution
//! model, and the `.pio.json` document model are peers, and a record that
//! crosses between them needs a home below all three.
//!
//! ```
//! use powerio_diag::{DiagnosticSeverity, DiagnosticStage, StructuredDiagnostic, render_line};
//!
//! let d = StructuredDiagnostic::new(
//!     "READ.DSS.INCLUDE_REFUSED",
//!     DiagnosticSeverity::Error,
//!     "redirect ../shared.dss: refused; the include escapes the case directory",
//! );
//! assert_eq!(d.stage(), Some(DiagnosticStage::Read));
//! assert!(render_line(&d).starts_with("READ.DSS.INCLUDE_REFUSED: "));
//! ```

pub mod category;
pub mod code;
pub mod collect;
pub mod record;
pub mod registry;
pub mod render;

pub use category::ErrorCategory;
pub use code::{DiagnosticCode, DiagnosticStage, code_is_well_formed};
pub use collect::Diagnostics;
pub use record::{DiagnosticSeverity, SourceRef, StructuredDiagnostic};
pub use registry::{CodeStatus, DiagnosticInfo, check_registry, check_scope_ownership};
pub use render::{render_line, render_lines};
