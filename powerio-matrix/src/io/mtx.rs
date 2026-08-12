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
    if is_exactly_symmetric(matrix) {
        write_symmetric_mtx(matrix, path)
    } else {
        sprs::io::write_matrix_market(path, matrix.view()).map_err(|e| Error::Mtx(e.to_string()))
    }
}

fn write_symmetric_mtx(matrix: &CsMat<f64>, path: &Path) -> Result<()> {
    let f = std::fs::File::create(path)?;
    let mut w = BufWriter::new(f);
    writeln!(w, "%%MatrixMarket matrix coordinate real symmetric")?;
    writeln!(w, "% written by powerio")?;

    // Two-pass: count entries first so the header can carry the exact nnz.
    let nnz = matrix
        .iter()
        .filter(|&(_, (i, j))| i >= j)
        .filter(|&(&v, _)| v != 0.0)
        .count();
    writeln!(w, "{} {} {}", matrix.rows(), matrix.cols(), nnz)?;

    for (&v, (i, j)) in matrix {
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
    let header = lines
        .next()
        .ok_or_else(|| Error::Mtx("empty vector file".into()))?;
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
    writeln!(w, "% written by powerio")?;
    writeln!(w, "{} 1", values.len())?;
    for v in values {
        writeln!(w, "{v:.16e}")?;
    }
    Ok(())
}

/// Whether the `symmetric` header would round trip: every stored entry has a
/// stored mirror holding the identical bits.
///
/// The writer emits only the lower triangle under that header, so a reader
/// reconstructs `a[j][i]` from `a[i][j]`. Deciding this on a tolerance meant a
/// matrix that was merely close went out as symmetric and came back changed by
/// up to the tolerance — a `Bp` built in BX mode with a small phase shifter is
/// asymmetric by exactly `2 g sin(theta) / t`, which sat under the old 1e-12.
/// Anything short of exact goes out as `general`, which stores both triangles.
fn is_exactly_symmetric(a: &CsMat<f64>) -> bool {
    if a.rows() != a.cols() {
        return false;
    }
    for (i, row) in a.outer_iterator().enumerate() {
        for (j, &v) in row.iter() {
            // Bit equality, so a mirrored pair differing only in the sign of
            // zero is `general` too: the symmetric form would not carry it.
            match a.get(j, i) {
                Some(&mirror) if mirror.to_bits() == v.to_bits() => {}
                _ => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use sprs::TriMat;

    use super::write_mtx;

    #[test]
    fn value_asymmetric_matrix_writes_general_mtx() {
        let mut tri = TriMat::new((2, 2));
        tri.add_triplet(0, 0, 2.0);
        tri.add_triplet(0, 1, -1.0);
        tri.add_triplet(1, 0, -2.0);
        tri.add_triplet(1, 1, 2.0);
        let matrix = tri.to_csr();

        let path = temp_path("value-asymmetric");
        write_mtx(&matrix, &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(
            text.lines().next().unwrap().ends_with("general"),
            "value-asymmetric matrices must not be written with a symmetric header"
        );
    }

    #[test]
    fn a_matrix_asymmetric_below_the_old_tolerance_writes_general() {
        // #292. The pair differs by 1e-15 relative, which the old 1e-12
        // tolerance called symmetric — so only the lower triangle went out and
        // a reader mirrored 3.0 back over the 3.000000000000001 that was
        // assembled. A `Bp` in BX mode with a small phase shifter is asymmetric
        // by exactly this little.
        let mut tri = TriMat::new((2, 2));
        tri.add_triplet(0, 0, 5.0);
        tri.add_triplet(0, 1, 3.000_000_000_000_001);
        tri.add_triplet(1, 0, 3.0);
        tri.add_triplet(1, 1, 5.0);
        let matrix = tri.to_csr();

        let path = temp_path("near-symmetric");
        write_mtx(&matrix, &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(
            text.lines().next().unwrap().ends_with("general"),
            "a matrix that is only nearly symmetric must carry both triangles:\n{text}"
        );
    }

    #[test]
    fn a_structurally_asymmetric_matrix_writes_general() {
        // The mirror is absent rather than unequal: the old check read it as a
        // stored 0.0 and compared equal whenever the entry was itself 0.0.
        let mut tri = TriMat::new((2, 2));
        tri.add_triplet(0, 0, 5.0);
        tri.add_triplet(0, 1, 0.0);
        tri.add_triplet(1, 1, 5.0);
        let matrix = tri.to_csr();

        let path = temp_path("structurally-asymmetric");
        write_mtx(&matrix, &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(
            text.lines().next().unwrap().ends_with("general"),
            "an unmirrored stored entry must not claim a symmetric header:\n{text}"
        );
    }

    #[test]
    fn an_exactly_symmetric_matrix_still_writes_symmetric() {
        let mut tri = TriMat::new((2, 2));
        tri.add_triplet(0, 0, 5.0);
        tri.add_triplet(0, 1, -3.0);
        tri.add_triplet(1, 0, -3.0);
        tri.add_triplet(1, 1, 5.0);
        let matrix = tri.to_csr();

        let path = temp_path("exactly-symmetric");
        write_mtx(&matrix, &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(
            text.lines().next().unwrap().ends_with("symmetric"),
            "an exactly symmetric matrix keeps the compact form:\n{text}"
        );
    }

    fn temp_path(stem: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        path.push(format!("powerio-{stem}-{nanos}.mtx"));
        path
    }
}
