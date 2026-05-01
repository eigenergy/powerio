//! File I/O: Matrix Market (`.mtx`), NumPy (`.npy`), and JSON metadata.

pub mod meta;
pub mod mtx;
pub mod npy;

pub use meta::{CaseMetadata, MatrixMetadata, write_meta_json};
pub use mtx::{read_mtx, write_mtx, write_vector_mtx};
pub use npy::{write_dense_npy, write_vector_npy};
