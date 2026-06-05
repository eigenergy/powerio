//! NumPy `.npy` writer (v2.0 header). Tested for shape compatibility
//! with `numpy.load`.

use std::io::Write;
use std::path::Path;

use sprs::CsMat;

use crate::Result;

/// Write a 1-D `f64` vector as a NumPy `.npy` file.
pub fn write_vector_npy(data: &[f64], path: impl AsRef<Path>) -> Result<()> {
    write_npy(data, &[data.len() as u64], path)
}

/// Write a sparse matrix as a **dense** `.npy`. Allocates `n*m` doubles —
/// only call this on small matrices (toy cases). For real grids prefer
/// `write_mtx`.
pub fn write_dense_npy(matrix: &CsMat<f64>, path: impl AsRef<Path>) -> Result<()> {
    let n = matrix.rows();
    let m = matrix.cols();
    let mut dense = vec![0.0; n * m];
    for (&v, (i, j)) in matrix.iter() {
        dense[i * m + j] = v;
    }
    write_npy(&dense, &[n as u64, m as u64], path)
}

fn write_npy(data: &[f64], shape: &[u64], path: impl AsRef<Path>) -> Result<()> {
    let shape_str: Vec<String> = shape.iter().map(u64::to_string).collect();
    let trailing_comma = if shape.len() == 1 { "," } else { "" };
    let header_dict = format!(
        "{{'descr': '<f8', 'fortran_order': False, 'shape': ({}{}), }}",
        shape_str.join(", "),
        trailing_comma,
    );

    // Pad header so (magic + version + len_field + header) is a multiple of 64.
    let mut header = header_dict + "\n";
    let prefix_len = 6 + 2 + 4; // magic(6) + version(2) + u32 length(4) for v2.0
    let total = prefix_len + header.len();
    let pad = (64 - (total % 64)) % 64;
    if pad > 0 {
        // Replace the trailing newline with spaces+newline.
        header.pop();
        header.push_str(&" ".repeat(pad));
        header.push('\n');
    }

    let mut f = std::fs::File::create(path)?;
    f.write_all(b"\x93NUMPY")?;
    f.write_all(&[0x02, 0x00])?;
    f.write_all(&(header.len() as u32).to_le_bytes())?;
    f.write_all(header.as_bytes())?;
    for &v in data {
        f.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}
