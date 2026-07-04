//! Streamed Matrix Market output for DC sensitivity matrices.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::Result;
use crate::indexed::IndexedNetwork;
use crate::matrix::sensitivity::for_each_ptdf_lodf_entry;
use crate::matrix::{SensitivityMetadata, SensitivityOptions};

/// Write PTDF and LODF Matrix Market files from the option based sensitivity
/// path and return the metadata for the same entries. The iterative solver path
/// streams retained coordinates through temp files, so it does not keep the
/// full sparse output in memory.
pub fn write_sensitivity_mtx_with_options(
    case: &IndexedNetwork,
    options: &SensitivityOptions,
    ptdf_path: impl AsRef<Path>,
    lodf_path: impl AsRef<Path>,
) -> Result<SensitivityMetadata> {
    let mut ptdf = CoordinateMtxWriter::new(ptdf_path.as_ref())?;
    let mut lodf = CoordinateMtxWriter::new(lodf_path.as_ref())?;

    let metadata = match for_each_ptdf_lodf_entry(
        case,
        options,
        |row, col, value| ptdf.write_entry(row, col, value),
        |row, col, value| lodf.write_entry(row, col, value),
    ) {
        Ok(metadata) => metadata,
        Err(err) => {
            ptdf.cleanup();
            lodf.cleanup();
            return Err(err);
        }
    };

    ptdf.finish(metadata.ptdf.rows, metadata.ptdf.cols)?;
    lodf.finish(metadata.lodf.rows, metadata.lodf.cols)?;
    Ok(metadata)
}

struct CoordinateMtxWriter {
    target_path: PathBuf,
    body_path: PathBuf,
    final_tmp_path: PathBuf,
    body: Option<BufWriter<File>>,
    nnz: usize,
}

impl CoordinateMtxWriter {
    fn new(target_path: &Path) -> Result<Self> {
        let body_path = temp_path(target_path, "body");
        let final_tmp_path = temp_path(target_path, "final");
        let body = BufWriter::new(File::create(&body_path)?);
        Ok(Self {
            target_path: target_path.to_path_buf(),
            body_path,
            final_tmp_path,
            body: Some(body),
            nnz: 0,
        })
    }

    fn write_entry(&mut self, row: usize, col: usize, value: f64) -> Result<()> {
        if value == 0.0 {
            return Ok(());
        }
        let body = self
            .body
            .as_mut()
            .expect("coordinate writer body is open before finish");
        writeln!(body, "{} {} {:.16e}", row + 1, col + 1, value)?;
        self.nnz += 1;
        Ok(())
    }

    fn finish(mut self, rows: usize, cols: usize) -> Result<()> {
        if let Some(mut body) = self.body.take() {
            body.flush()?;
        }

        let mut out = BufWriter::new(File::create(&self.final_tmp_path)?);
        writeln!(out, "%%MatrixMarket matrix coordinate real general")?;
        writeln!(out, "% written by powerio")?;
        writeln!(out, "{rows} {cols} {}", self.nnz)?;
        let mut body = BufReader::new(File::open(&self.body_path)?);
        std::io::copy(&mut body, &mut out)?;
        out.flush()?;

        std::fs::rename(&self.final_tmp_path, &self.target_path)?;
        let _ = std::fs::remove_file(&self.body_path);
        Ok(())
    }

    fn cleanup(&mut self) {
        if let Some(mut body) = self.body.take() {
            let _ = body.flush();
        }
        let _ = std::fs::remove_file(&self.body_path);
        let _ = std::fs::remove_file(&self.final_tmp_path);
    }
}

fn temp_path(target_path: &Path, suffix: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let name = target_path
        .file_name()
        .map_or_else(|| "matrix".into(), |name| name.to_string_lossy());
    target_path.with_file_name(format!(".{name}.{pid}.{nanos}.{suffix}.tmp"))
}
