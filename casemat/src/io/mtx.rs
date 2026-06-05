//! Matrix Market I/O.
//!
//! `sprs::io::write_matrix_market_sym` writes the *upper* triangle, but the
//! Matrix Market spec calls for the *lower* triangle (i ≥ j). To stay
//! compatible with strict readers (e.g. `fast_matrix_market`), we hand roll
//! the symmetric writer. We delegate to `sprs` for general (non symmetric)
//! output and for reading.

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
    writeln!(w, "% written by casemat")?;

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

/// Read a dense vector written by [`write_vector_mtx`] (`array real general`):
/// `%`-comment lines, a `<len> 1` dimensions line, then one value per line.
pub fn read_vector_mtx(path: impl AsRef<Path>) -> Result<Vec<f64>> {
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines().filter(|l| !l.starts_with('%'));
    let header = lines.next().ok_or_else(|| Error::Mtx("empty vector file".into()))?;
    let len: usize = header
        .split_whitespace()
        .next()
        .and_then(|t| t.parse().ok())
        .ok_or_else(|| Error::Mtx(format!("bad vector dimensions line: {header:?}")))?;
    let values = lines
        .take(len)
        .map(|l| {
            l.trim()
                .parse::<f64>()
                .map_err(|_| Error::Mtx(format!("bad vector entry: {l:?}")))
        })
        .collect::<Result<Vec<_>>>()?;
    if values.len() != len {
        return Err(Error::Mtx(format!(
            "expected {len} entries, got {}",
            values.len()
        )));
    }
    Ok(values)
}

/// Write a dense vector as Matrix Market `array real general`.
pub fn write_vector_mtx(values: &[f64], path: impl AsRef<Path>) -> Result<()> {
    let f = std::fs::File::create(path)?;
    let mut w = BufWriter::new(f);
    writeln!(w, "%%MatrixMarket matrix array real general")?;
    writeln!(w, "% written by casemat")?;
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
