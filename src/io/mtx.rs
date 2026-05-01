//! Matrix Market I/O.
//!
//! `sprs::io::write_matrix_market_sym` writes the *upper* triangle, but the
//! Matrix Market spec calls for the *lower* triangle (i ≥ j). To stay
//! compatible with strict readers (notably `fast_matrix_market` used by
//! the Scalable Approximate Cholesky solver), we hand roll the symmetric
//! writer. We delegate to `sprs` for general (non symmetric) output and
//! for reading.

use std::io::{BufWriter, Write};
use std::path::Path;

use sprs::CsMat;

use crate::{Error, Result};

pub fn write_mtx(matrix: &CsMat<f64>, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    if is_structurally_symmetric(matrix) {
        write_symmetric_mtx(matrix, path)
    } else {
        sprs::io::write_matrix_market(path, matrix.view()).map_err(|e| Error::Mtx(e.to_string()))
    }
}

fn write_symmetric_mtx(matrix: &CsMat<f64>, path: &Path) -> Result<()> {
    let f = std::fs::File::create(path)?;
    let mut w = BufWriter::new(f);
    writeln!(w, "%%MatrixMarket matrix coordinate real symmetric")?;
    writeln!(w, "% written by mpower-bmat")?;

    // Two-pass: count entries first so the header can carry the exact nnz.
    let nnz = matrix
        .iter()
        .filter(|&(_, (i, j))| i >= j)
        .filter(|&(&v, _)| v != 0.0)
        .count();
    writeln!(w, "{} {} {}", matrix.rows(), matrix.cols(), nnz)?;

    for (&v, (i, j)) in matrix.iter() {
        if i < j || v == 0.0 {
            continue;
        }
        writeln!(w, "{} {} {:.16e}", i + 1, j + 1, v)?;
    }
    Ok(())
}

/// Read a Matrix Market file into a CSR matrix.
pub fn read_mtx(path: impl AsRef<Path>) -> Result<CsMat<f64>> {
    let tri: sprs::TriMat<f64> =
        sprs::io::read_matrix_market(path).map_err(|e| Error::Mtx(e.to_string()))?;
    Ok(tri.to_csr())
}

/// Write a dense vector as Matrix Market `array real general`.
pub fn write_vector_mtx(values: &[f64], path: impl AsRef<Path>) -> Result<()> {
    let f = std::fs::File::create(path)?;
    let mut w = BufWriter::new(f);
    writeln!(w, "%%MatrixMarket matrix array real general")?;
    writeln!(w, "% written by mpower-bmat")?;
    writeln!(w, "{} 1", values.len())?;
    for v in values {
        writeln!(w, "{v:.16e}")?;
    }
    Ok(())
}

fn is_structurally_symmetric(a: &CsMat<f64>) -> bool {
    if a.rows() != a.cols() {
        return false;
    }
    for (i, row) in a.outer_iterator().enumerate() {
        for (j, &v) in row.iter() {
            let mirror = a.get(j, i).copied().unwrap_or(0.0);
            if (v - mirror).abs() > 1e-12 {
                return false;
            }
        }
    }
    true
}
