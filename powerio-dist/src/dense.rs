//! Small dense matrix algebra shared by every conductor sized calculation:
//! the series impedance inverses of lines and transformers, the potential
//! coefficient inverses of overhead geometry, and Kron reduction. The kernel
//! is [`powerio_core::dense`], the one dense elimination; this module
//! re-exports it.

pub use powerio_core::dense::{DensePivot, DenseScalar, invert_dense_matrix};
pub(crate) use powerio_core::dense::{DenseRefusal, eliminate_dense_matrix};
